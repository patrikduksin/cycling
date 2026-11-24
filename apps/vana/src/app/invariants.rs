use super::Runtime;
use super::format::Format;
use super::power::Sheet;
use crate::ride::current::Request;
use crate::ride::medium::Medium;
use crate::ride::recorder::Action;
use crate::ride::recorder::invariants as recorder;
use crate::screens::workout::Page;

invariants! {
    pub enum Rule of runtime: Runtime {
        PhoneWhileRecording: "phone session open while recording"
            if runtime.phone_status().is_some() && runtime.recorder_busy(),
        TurningOffWhileRecording: "turning-off frame shown while recording"
            if matches!(runtime.power.sheet, Sheet::TurningOff { .. }) && runtime.recorder_busy(),
        ConfirmOffWhileRecording: "turn-off question shown while recording"
            if matches!(runtime.power.sheet, Sheet::Confirm { .. }) && runtime.recorder_busy(),
        DoneWhileRiding: "ride done shown while riding"
            if runtime.navigation.page() == Page::Done
                && runtime.ride.active().riding()
                && runtime.ride.active().pending_action() != Some(Action::Finish),
        TwoRidingRides: "both journals riding"
            if runtime.ride.internal().riding() && runtime.ride.bulk().riding(),
        HostTokenOwned: "a host operation waits without exactly one owner of its token"
            if runtime.host_token_unowned(),
        PageWhileWiping: "a page other than FORMAT or the brand shown while a wipe runs"
            if matches!(runtime.format, Format::Running { .. })
                && !matches!(runtime.navigation.page(), Page::Format | Page::Boot),
        EditingOffSettings: "a Settings number edited off the Settings page"
            if runtime.navigation.editing() && runtime.navigation.page() != Page::Settings,
        TurningOffUnsynced: "turning off while the phone has not confirmed every ride"
            if matches!(runtime.power.sheet, Sheet::TurningOff { .. } | Sheet::Requested { .. })
                && runtime.holds_unsynced(),
        UnprotectedVolatileRide: "a volatile journal's open ride is not protected"
            if runtime
                .ride
                .volatile_ride()
                .is_some_and(|id| !runtime.sync.protects(Medium::Internal, id)),
        VolatileError: "a volatile journal in error"
            if runtime.ride.volatile(Medium::Internal)
                && runtime.ride.internal().status() == crate::ride::log::Status::Error,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Broken {
    Rule(Rule),
    Recorder(Medium, recorder::Rule),
}

impl core::fmt::Display for Broken {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Rule(rule) => write!(f, "{rule}"),
            Self::Recorder(medium, rule) => write!(f, "{} recorder: {rule}", medium.name()),
        }
    }
}

impl Runtime {
    fn host_token_unowned(&self) -> bool {
        let host = match self.ride.host() {
            Request::Pending(token) => Some(token),
            Request::Idle | Request::Completed(_) | Request::Declined(_) => None,
        };
        let wiping = match self.format {
            Format::Running { clear, .. } => clear,
            _ => None,
        };
        let owners = |token| {
            usize::from(wiping == Some(token))
                + usize::from(self.sync.asked() == Some(token))
                + [Medium::Internal, Medium::Bulk]
                    .into_iter()
                    .filter(|&medium| self.ride.recorder(medium).outstanding_token() == Some(token))
                    .count()
        };
        let asked = self.sync.asked();
        host.is_some_and(|token| owners(token) != 1)
            || wiping.is_some_and(|token| host != Some(token))
            || asked.is_some_and(|token| host != Some(token))
    }

    pub fn broken(&self) -> Option<Broken> {
        [Medium::Internal, Medium::Bulk]
            .into_iter()
            .find_map(|medium| {
                recorder::Rule::of(self.ride.recorder(medium))
                    .map(|rule| Broken::Recorder(medium, rule))
            })
            .or_else(|| Rule::of(self).map(Broken::Rule))
    }

    #[cfg(any(test, feature = "invariants"))]
    #[doc(hidden)]
    pub fn break_a_rule(&mut self) {
        self.ride.set_host(Request::Pending(9));
    }
}
