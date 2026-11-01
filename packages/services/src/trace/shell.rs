//! The shell's own devices as a C606 records them: the panel, the backlight
//! and the internal flash. The shell owns each device, so its wrapper notes
//! what the trace needs in a [`ShellLog`] it shares with composition, which
//! records the log after each pass and each command. The simulator's devices
//! record the same events from their own state
//! (`tools/simulator/src/devices.rs` and `storage.rs`), numbered alike:
//! attempts count from zero each pass.
use super::frozen::{CompanionView, Record};
use super::{Event, Output, StorageError, StorageFault, StorageOp};
use core::cell::{Cell, RefCell};
use device_api::display::{Display, Format, Geometry};
use device_api::observation::{Availability, Error};
use device_api::power::Power;
use device_api::storage::{AccessError, OwnedFlash, Region};

/// Faults one pass holds for the trace; more are dropped.
const FAULTS: usize = 8;

/// The attempts an output refused this pass, by number.
#[derive(Clone, Copy, Default)]
struct Refusals {
    attempts: u8,
    refused: u32,
}

impl Refusals {
    fn attempt(&mut self, refused: bool) {
        if refused && self.attempts < 32 {
            self.refused |= 1 << self.attempts;
        }
        self.attempts = self.attempts.saturating_add(1);
    }
}

/// What the shell's devices did since the pass began, until composition
/// records it, and the backlight and battery the pass froze for the shell.
#[derive(Default)]
pub struct ShellLog {
    panel: Cell<Refusals>,
    backlight: Cell<Refusals>,
    availability: Cell<Option<Availability>>,
    battery: Cell<Option<(u8, u16, u64)>>,
    flash_attempts: Cell<u32>,
    faults: RefCell<heapless::Vec<StorageFault, FAULTS>>,
}

impl ShellLog {
    /// A new pass: attempts count from zero, and the shell reads this
    /// backlight availability and the battery of `companion` until the
    /// next pass, as the simulator's shell does.
    pub fn begin_pass(&self, backlight: Availability, companion: &CompanionView) {
        for refusals in [&self.panel, &self.backlight] {
            refusals.set(Refusals {
                attempts: 0,
                ..refusals.get()
            });
        }
        self.flash_attempts.set(0);
        self.availability.set(Some(backlight));
        self.battery.set(super::frozen::battery(companion));
    }

    /// Record what the devices refused or failed since the last call.
    pub fn record(&self, record: &RefCell<impl Record + ?Sized>) {
        let mut record = record.borrow_mut();
        for (output, cell) in [
            (Output::Panel, &self.panel),
            (Output::Backlight, &self.backlight),
        ] {
            let refusals = cell.get();
            for attempt in (0..32u8).filter(|attempt| refusals.refused & (1 << attempt) != 0) {
                record.record(&Event::Refused { output, attempt });
            }
            cell.set(Refusals {
                refused: 0,
                ..refusals
            });
        }
        for fault in self.faults.borrow_mut().drain(..) {
            record.record(&Event::StorageFault(fault));
        }
    }

    fn fault(&self, region: Region, op: StorageOp, at: usize, error: StorageError) {
        let attempt = self.flash_attempts.get();
        self.flash_attempts.set(attempt + 1);
        let _ = self.faults.borrow_mut().push(StorageFault {
            medium: region.into(),
            attempt,
            op,
            at: at as u64,
            landed: 0,
            error,
        });
    }

    fn succeeded(&self) {
        self.flash_attempts.set(self.flash_attempts.get() + 1);
    }
}

/// The panel, its refused frames noted for the trace.
pub struct ShellDisplay<D> {
    inner: D,
    log: &'static ShellLog,
}

impl<D> ShellDisplay<D> {
    pub fn new(inner: D, log: &'static ShellLog) -> Self {
        Self { inner, log }
    }
}

impl<D: Display> Display for ShellDisplay<D> {
    fn availability(&self) -> Availability {
        self.inner.availability()
    }
    fn geometry(&self) -> Geometry {
        self.inner.geometry()
    }
    fn format(&self) -> Format {
        self.inner.format()
    }
    fn submit(&mut self, row: impl FnMut(usize, &mut [u16])) -> Result<(), Error> {
        let result = self.inner.submit(row);
        let mut panel = self.log.panel.get();
        panel.attempt(result.is_err());
        self.log.panel.set(panel);
        result
    }
}

/// The backlight: its availability and the battery as the pass froze them,
/// and its refused writes noted for the trace.
pub struct ShellPower<P> {
    inner: P,
    log: &'static ShellLog,
}

impl<P> ShellPower<P> {
    pub fn new(inner: P, log: &'static ShellLog) -> Self {
        Self { inner, log }
    }
}

impl<P: Power> Power for ShellPower<P> {
    fn availability(&self) -> Availability {
        self.log
            .availability
            .get()
            .unwrap_or_else(|| self.inner.availability())
    }
    fn battery(&self) -> Option<(u8, u16, u64)> {
        self.log.battery.get()
    }
    fn brightness(&mut self, percent: u8) -> Result<(), Error> {
        let result = self.inner.brightness(percent);
        let mut backlight = self.log.backlight.get();
        backlight.attempt(result.is_err());
        self.log.backlight.set(backlight);
        result
    }
}

/// An internal flash error as the trace names it.
pub trait FlashError {
    fn storage_error(&self) -> StorageError;
}

impl FlashError for Error {
    fn storage_error(&self) -> StorageError {
        StorageError::from_flash(*self)
    }
}

impl<E> FlashError for AccessError<E> {
    fn storage_error(&self) -> StorageError {
        match self {
            Self::Unavailable => StorageError::Unavailable,
            Self::OutOfBounds | Self::Unaligned => StorageError::Invalid,
            Self::Device(_) => StorageError::Failed,
        }
    }
}

/// The internal flash, each operation that failed noted for the trace as a
/// storage fault. What a failed write left on the flash is unknown here, so
/// the fault says nothing landed.
pub struct ShellFlash<B> {
    inner: B,
    log: &'static ShellLog,
}

impl<B> ShellFlash<B> {
    pub fn new(inner: B, log: &'static ShellLog) -> Self {
        Self { inner, log }
    }

    fn noted<T, E: FlashError>(
        &self,
        region: Region,
        op: StorageOp,
        at: usize,
        result: Result<T, E>,
    ) -> Result<T, E> {
        match &result {
            Ok(_) => self.log.succeeded(),
            Err(error) => self.log.fault(region, op, at, error.storage_error()),
        }
        result
    }
}

impl<B: OwnedFlash> OwnedFlash for ShellFlash<B>
where
    B::Error: FlashError,
{
    type Error = B::Error;
    fn availability(&self, region: Region) -> Availability {
        self.inner.availability(region)
    }
    fn geometry(&self, region: Region) -> device_api::storage::Geometry {
        self.inner.geometry(region)
    }
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), B::Error> {
        let result = self.inner.read(region, offset, output);
        self.noted(region, StorageOp::Read, offset, result)
    }
    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), B::Error> {
        let result = self.inner.program(region, offset, bytes);
        self.noted(region, StorageOp::Write, offset, result)
    }
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), B::Error> {
        let result = self.inner.erase(region, offset, length);
        self.noted(region, StorageOp::Erase, offset, result)
    }
}
