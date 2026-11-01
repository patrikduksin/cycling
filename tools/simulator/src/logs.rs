//! The simulated device's logger: warnings and errors wait, per thread as
//! each scenario runs on its own, until the session records them into its
//! trace.
use firmware_services::trace::Level;
use std::cell::RefCell;

/// Records kept between passes; more are dropped.
const KEPT: usize = 64;

struct Record {
    level: Level,
    target: String,
    text: String,
}

thread_local! {
    static WAITING: RefCell<Vec<Record>> = const { RefCell::new(Vec::new()) };
}

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        let level = match record.level() {
            log::Level::Error => Level::Error,
            log::Level::Warn => Level::Warn,
            _ => return,
        };
        WAITING.with(|waiting| {
            let mut waiting = waiting.borrow_mut();
            if waiting.len() < KEPT {
                waiting.push(Record {
                    level,
                    target: record.target().into(),
                    text: record.args().to_string(),
                });
            }
        });
    }

    fn flush(&self) {}
}

/// Start keeping warnings and errors; later calls change nothing.
pub fn install() {
    static LOGGER: Logger = Logger;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

/// Hand over this thread's waiting records, oldest first.
pub fn drain(mut each: impl FnMut(Level, &str, &str)) {
    let waiting = WAITING.with(|waiting| core::mem::take(&mut *waiting.borrow_mut()));
    for record in &waiting {
        each(record.level, &record.target, &record.text);
    }
}
