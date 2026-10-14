//! Power and command admission shared by the BLE and Wi-Fi radio owners.
//!
//! Each radio has one owner task. Callers submit at most one pending command
//! and power requests through [`Control`]; the owner takes the command, reports
//! completion, and acknowledges power transitions. Nothing is replayed: a
//! taken command is gone, and an accepted command or power target can only be
//! superseded, never queued behind another.
//!
//! `uncertain` is the fail-closed rule for radios whose public driver APIs
//! cannot prove a cancelled scan, connection or disconnect has stopped. Once
//! set it is never cleared: every later suspend or resume reports
//! [`PeripheralState::Failed`], so the power owner never claims quiescence for
//! a radio that may still be active. Only a reboot clears it.

use device_api::connectivity::ControlError;
use device_api::connectivity::ControlStatus;
use device_api::connectivity::Operation;
use device_api::observation::Error;
use device_api::power::PeripheralState;

/// Bounded number of remembered scan results per radio.
pub const DISCOVERIES: usize = 8;

pub struct Control<Op> {
    ready: bool,
    suspended: bool,
    power: PeripheralState,
    status: ControlStatus,
    command: Option<Op>,
    uncertain: bool,
}

impl<Op> Default for Control<Op> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Op> Control<Op> {
    pub const fn new() -> Self {
        Self {
            ready: false,
            suspended: false,
            power: PeripheralState::Running,
            status: ControlStatus::new(),
            command: None,
            uncertain: false,
        }
    }
    /// The owner task is running and accepts requests.
    pub fn set_ready(&mut self) {
        self.ready = true;
    }
    pub fn ready(&self) -> bool {
        self.ready
    }
    /// The current power target, independent of whether it has been reached.
    pub fn suspended(&self) -> bool {
        self.suspended
    }
    pub fn power(&self) -> PeripheralState {
        self.power
    }
    pub fn status(&self) -> ControlStatus {
        self.status
    }
    pub fn has_command(&self) -> bool {
        self.command.is_some()
    }
    /// Owner side: claim the pending command. It is never delivered twice.
    pub fn take_command(&mut self) -> Option<Op> {
        self.command.take()
    }
    pub fn uncertain(&self) -> bool {
        self.uncertain
    }
    /// Record that the radio may still be active. See the module rules.
    pub fn mark_uncertain(&mut self) {
        self.uncertain = true;
    }

    /// Admit a new power target. Suspend is refused while a command is pending
    /// so the owner never abandons an accepted operation. Returns whether the
    /// target changed; an unchanged target leaves the reported state intact.
    pub fn power_request(&mut self, suspend: bool) -> Result<bool, Error> {
        if !self.ready
            || (suspend && (self.status.operation == Operation::Pending || self.command.is_some()))
        {
            return Err(Error::Unavailable);
        }
        if self.suspended == suspend {
            return Ok(false);
        }
        self.suspended = suspend;
        self.power = PeripheralState::Pending;
        Ok(true)
    }
    /// Owner side: report the outcome for `suspend`. A result for a target that
    /// was superseded in the meantime is ignored.
    pub fn power_complete(&mut self, suspend: bool, state: PeripheralState) {
        if self.suspended == suspend {
            self.power = state;
        }
    }
    /// The state a finished suspension reports. `quiet` is the owner's own
    /// evidence of inactivity; uncertainty or a stopped owner overrides it.
    pub fn suspend_outcome(&self, quiet: bool) -> PeripheralState {
        if quiet && !self.uncertain && self.ready {
            PeripheralState::Quiescent
        } else {
            PeripheralState::Failed
        }
    }
    /// Owner side: finish a pending resume. Returns the resulting state.
    pub fn complete_resume(&mut self) -> PeripheralState {
        if !self.suspended && self.power == PeripheralState::Pending {
            let state = if self.uncertain {
                PeripheralState::Failed
            } else {
                PeripheralState::Running
            };
            self.power_complete(false, state);
        }
        self.power
    }

    /// Accept one command while the radio is running and nothing is pending.
    pub fn request(&mut self, operation: Op) -> Result<(), Error> {
        if !self.ready
            || self.suspended
            || self.power != PeripheralState::Running
            || self.status.operation == Operation::Pending
            || self.command.is_some()
        {
            return Err(Error::Unavailable);
        }
        self.status.sequence = self.status.sequence.wrapping_add(1);
        self.status.operation = Operation::Pending;
        self.status.error = None;
        self.command = Some(operation);
        Ok(())
    }
    /// Report the outcome of the current operation.
    pub fn complete(&mut self, error: Option<ControlError>) {
        self.status.operation = if error.is_some() {
            Operation::Failed
        } else {
            Operation::Completed
        };
        self.status.error = error;
    }
    /// Report a requested operation's outcome. Automatic reconnects have no
    /// requester, and a newer accepted command owns the status instead.
    pub fn finish(&mut self, automatic: bool, error: Option<ControlError>) {
        if !automatic && self.command.is_none() {
            self.complete(error);
        }
    }
    /// The owner can no longer serve requests: refuse further admission, drop
    /// any unclaimed command and report the failure.
    pub fn stop(&mut self, error: ControlError) {
        self.ready = false;
        self.power = PeripheralState::Failed;
        self.command = None;
        self.complete(Some(error));
    }
}

#[cfg(feature = "hardware")]
pub use shared::Shared;

#[cfg(feature = "hardware")]
mod shared {
    use super::Control;
    use super::DISCOVERIES;
    use core::cell::RefCell;
    use critical_section::Mutex;
    use device_api::connectivity::ControlError;
    use device_api::connectivity::ControlStatus;
    use device_api::observation::Error;
    use device_api::power::PeripheralState;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::signal::Signal;

    /// Interrupt-safe [`Control`] plus the owner's wake-up and scan results.
    ///
    /// `wake` only tells the single owner task to re-examine state. It carries
    /// no payload, so a stale or coalesced signal can never replay a request.
    pub struct Shared<Op, D> {
        control: Mutex<RefCell<Control<Op>>>,
        discoveries: Mutex<RefCell<[Option<D>; DISCOVERIES]>>,
        wake: Signal<CriticalSectionRawMutex, ()>,
    }

    impl<Op: Copy, D: Copy> Default for Shared<Op, D> {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<Op: Copy, D: Copy> Shared<Op, D> {
        pub const fn new() -> Self {
            Self {
                control: Mutex::new(RefCell::new(Control::new())),
                discoveries: Mutex::new(RefCell::new([None; DISCOVERIES])),
                wake: Signal::new(),
            }
        }
        pub fn with<R>(&self, f: impl FnOnce(&mut Control<Op>) -> R) -> R {
            critical_section::with(|cs| f(&mut self.control.borrow_ref_mut(cs)))
        }
        pub fn set_ready(&self) {
            self.with(Control::set_ready);
        }
        pub fn ready(&self) -> bool {
            self.with(|c| c.ready())
        }
        pub fn suspended(&self) -> bool {
            self.with(|c| c.suspended())
        }
        pub fn power(&self) -> PeripheralState {
            self.with(|c| c.power())
        }
        pub fn status(&self) -> ControlStatus {
            self.with(|c| c.status())
        }
        pub fn has_command(&self) -> bool {
            self.with(|c| c.has_command())
        }
        pub fn take_command(&self) -> Option<Op> {
            self.with(Control::take_command)
        }
        pub fn uncertain(&self) -> bool {
            self.with(|c| c.uncertain())
        }
        pub fn mark_uncertain(&self) {
            self.with(Control::mark_uncertain);
        }
        pub fn finish(&self, automatic: bool, error: Option<ControlError>) {
            self.with(|c| c.finish(automatic, error));
        }
        pub fn complete(&self, error: Option<ControlError>) {
            self.with(|c| c.complete(error));
        }
        pub fn stop(&self, error: ControlError) {
            self.with(|c| c.stop(error));
        }
        pub fn complete_resume(&self) -> PeripheralState {
            self.with(Control::complete_resume)
        }
        pub fn suspend_outcome(&self, quiet: bool) -> PeripheralState {
            self.with(|c| c.suspend_outcome(quiet))
        }
        pub fn power_complete(&self, suspend: bool, state: PeripheralState) {
            self.with(|c| c.power_complete(suspend, state));
        }
        /// Admit a power target; `on_change` runs in the same critical section
        /// when the target actually changes.
        pub fn power_request(&self, suspend: bool, on_change: impl FnOnce()) -> Result<(), Error> {
            let changed = self.with(|c| {
                let changed = c.power_request(suspend)?;
                if changed {
                    on_change();
                }
                Ok(changed)
            })?;
            if changed {
                self.wake.signal(());
            }
            Ok(())
        }
        pub fn request(&self, operation: Op) -> Result<(), Error> {
            self.with(|c| c.request(operation))?;
            self.wake.signal(());
            Ok(())
        }
        /// Wake the owner for state it tracks outside [`Control`].
        pub fn notify(&self) {
            self.wake.signal(());
        }
        /// Owner side: wait until `condition` holds, re-checking on each wake.
        pub async fn wait_until(&self, condition: impl Fn(&Control<Op>) -> bool) {
            while !self.with(|c| condition(c)) {
                self.wake.wait().await;
            }
        }
        /// Owner side: wait for a new command or a suspend target.
        pub async fn work_pending(&self) {
            self.wait_until(|c| c.suspended() || c.has_command()).await
        }
        /// Owner side: wait for any request or notification.
        pub async fn changed(&self) {
            self.wake.wait().await
        }
        pub fn discoveries(&self) -> [Option<D>; DISCOVERIES] {
            critical_section::with(|cs| *self.discoveries.borrow_ref(cs))
        }
        pub fn update_discoveries<R>(
            &self,
            f: impl FnOnce(&mut [Option<D>; DISCOVERIES]) -> R,
        ) -> R {
            critical_section::with(|cs| f(&mut self.discoveries.borrow_ref_mut(cs)))
        }
        pub fn clear_discoveries(&self) {
            self.update_discoveries(|d| *d = [None; DISCOVERIES]);
        }
    }
}
