//! The simulated device's logger: warnings and errors wait, per thread as
//! each scenario runs on its own, until the session records them into its
//! trace.
use crate::compare::Tap;
use firmware_services::trace::Level;
use std::cell::RefCell;
use std::sync::Once;

const MAX_WAITING: usize = 64;

#[derive(Clone, Copy)]
pub(crate) enum Line {
    Kept(Level),
    Info,
}

struct Record {
    level: Level,
    target: String,
    text: String,
}

struct Device {
    tap: Tap,
    now_ms: u64,
}

thread_local! {
    static WAITING: RefCell<Vec<Record>> = const { RefCell::new(Vec::new()) };
    static DEVICE: RefCell<Option<Device>> = const { RefCell::new(None) };
}

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        let line = match record.level() {
            log::Level::Error => Line::Kept(Level::Error),
            log::Level::Warn => Line::Kept(Level::Warn),
            log::Level::Info => Line::Info,
            _ => return,
        };
        let vana = record
            .module_path()
            .is_some_and(|path| path == "vana" || path.starts_with("vana::"));
        let text = DEVICE.with(|device| {
            let device = device.borrow();
            let device = device
                .as_ref()
                .filter(|_| matches!(line, Line::Kept(_)) || vana)?;
            let text = record.args().to_string();
            device.tap.log(device.now_ms, line, record.target(), &text);
            Some(text)
        });
        let Line::Kept(level) = line else {
            return;
        };
        WAITING.with(|waiting| {
            let mut waiting = waiting.borrow_mut();
            if waiting.len() < MAX_WAITING {
                waiting.push(Record {
                    level,
                    target: record.target().into(),
                    text: text.unwrap_or_else(|| record.args().to_string()),
                });
            }
        });
    }

    fn flush(&self) {}
}

/// Start keeping warnings and errors; later calls change nothing.
pub fn install() {
    static LOGGER: Logger = Logger;
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(log::LevelFilter::Warn);
        }
    });
}

pub(crate) fn enable_info() {
    log::set_max_level(log::LevelFilter::Info);
}

pub(crate) fn enter_device(tap: Option<&Tap>, now_ms: u64) {
    let device = tap.map(|tap| Device {
        tap: tap.clone(),
        now_ms,
    });
    DEVICE.with(|current| *current.borrow_mut() = device);
}

/// Hand over this thread's waiting records, oldest first.
pub fn drain(mut each: impl FnMut(Level, &str, &str)) {
    let waiting = WAITING.with(|waiting| core::mem::take(&mut *waiting.borrow_mut()));
    for record in &waiting {
        each(record.level, &record.target, &record.text);
    }
}
