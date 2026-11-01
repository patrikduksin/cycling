//! Firmware log adapter. The USB owner drains complete records outside the lock.
use core::cell::RefCell;
use core::fmt;
use core::fmt::Write;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;
use critical_section::Mutex;
use firmware_console::log_record::Line;
use firmware_console::log_record::Queue;
use firmware_console::log_record::Record;

static QUEUE: Mutex<RefCell<Queue>> = Mutex::new(RefCell::new(Queue::new()));
static BOOT: AtomicU32 = AtomicU32::new(0);
static ENQUEUES: AtomicU32 = AtomicU32::new(0);
static ENQUEUE_US: AtomicU32 = AtomicU32::new(0);
static MAX_ENQUEUE_US: AtomicU32 = AtomicU32::new(0);
static LOGGER: Logger = Logger;
struct Logger;

// Bounded UTF-8 prefix. A suffix makes truncation visible without invalid JSON.
struct Message {
    bytes: [u8; 160],
    len: usize,
    truncated: bool,
}
impl Write for Message {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.truncated {
            return Err(fmt::Error);
        }
        let mut count = s.len().min(self.bytes.len() - self.len);
        while !s.is_char_boundary(count) {
            count -= 1;
        }
        self.bytes[self.len..self.len + count].copy_from_slice(&s.as_bytes()[..count]);
        self.len += count;
        if count < s.len() {
            self.truncated = true;
            return Err(fmt::Error);
        }
        Ok(())
    }
}
impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record<'_>) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let mut msg = Message {
            bytes: [0; 160],
            len: 0,
            truncated: false,
        };
        let _ = write!(msg, "{}", r.args());
        #[cfg(feature = "cycling")]
        if r.level() <= log::Level::Warn {
            let level = match r.level() {
                log::Level::Error => firmware_services::trace::Level::Error,
                _ => firmware_services::trace::Level::Warn,
            };
            let text = core::str::from_utf8(&msg.bytes[..msg.len]).unwrap_or("");
            critical_section::with(|cs| {
                if let Some(kept) = KEPT.borrow_ref_mut(cs).as_mut() {
                    kept.push(level, r.target(), text);
                }
            });
        }
        event(
            r.level().as_str(),
            r.target(),
            if msg.truncated {
                "message_truncated"
            } else {
                "message"
            },
            core::str::from_utf8(&msg.bytes[..msg.len]).unwrap_or("format_error"),
        );
    }
    fn flush(&self) {} // Never wait on a host.
}

pub fn init(boot: u32) {
    BOOT.store(boot, Ordering::Relaxed);
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
pub fn event(level: &str, component: &str, event: &str, message: &str) {
    let started = embassy_time::Instant::now();
    let record = Record {
        r#type: "log",
        boot: BOOT.load(Ordering::Relaxed),
        seq: 0,
        ms: embassy_time::Instant::now().as_millis(),
        level,
        component,
        event,
        message,
        lost: 0,
    };
    critical_section::with(|cs| QUEUE.borrow(cs).borrow_mut().push(record));
    let elapsed = started.elapsed().as_micros().min(u64::from(u32::MAX)) as u32;
    ENQUEUES.fetch_add(1, Ordering::Relaxed);
    let _ = ENQUEUE_US.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        Some(v.saturating_add(elapsed))
    });
    MAX_ENQUEUE_US.fetch_max(elapsed, Ordering::Relaxed);
}
#[cfg(feature = "cycling")]
struct Kept {
    bytes: &'static mut [u8],
    len: usize,
}

#[cfg(feature = "cycling")]
static KEPT: Mutex<RefCell<Option<Kept>>> = Mutex::new(RefCell::new(None));

#[cfg(feature = "cycling")]
impl Kept {
    fn push(&mut self, level: firmware_services::trace::Level, target: &str, text: &str) {
        let (target, text) = (
            &target[..target.len().min(255)],
            &text[..text.len().min(255)],
        );
        let end = self.len + 3 + target.len() + text.len();
        let Some(room) = self.bytes.get_mut(self.len..end) else {
            return;
        };
        let level = matches!(level, firmware_services::trace::Level::Error) as u8;
        room[..3].copy_from_slice(&[level, target.len() as u8, text.len() as u8]);
        room[3..3 + target.len()].copy_from_slice(target.as_bytes());
        room[3 + target.len()..].copy_from_slice(text.as_bytes());
        self.len = end;
    }

    fn each(&self, each: &mut impl FnMut(firmware_services::trace::Level, &str, &str)) {
        use firmware_services::trace::Level;
        let mut at = 0;
        while at + 3 <= self.len {
            let [level, target, text] = [self.bytes[at], self.bytes[at + 1], self.bytes[at + 2]];
            let target_at = at + 3;
            let text_at = target_at + usize::from(target);
            at = text_at + usize::from(text);
            let target = core::str::from_utf8(&self.bytes[target_at..text_at]).unwrap_or("");
            let text = core::str::from_utf8(&self.bytes[text_at..at]).unwrap_or("");
            let level = if level == 1 {
                Level::Error
            } else {
                Level::Warn
            };
            each(level, target, text);
        }
    }
}

/// Keep warnings and errors in `bytes` for the trace from now on.
#[cfg(feature = "cycling")]
pub fn keep_warnings(bytes: &'static mut [u8]) {
    critical_section::with(|cs| *KEPT.borrow_ref_mut(cs) = Some(Kept { bytes, len: 0 }));
}

/// Hand over the warnings and errors kept since the last call, oldest
/// first. The buffer leaves the lock while they are recorded; a warning
/// logged meanwhile is dropped.
#[cfg(feature = "cycling")]
pub fn drain_warnings(mut each: impl FnMut(firmware_services::trace::Level, &str, &str)) {
    let Some(kept) = critical_section::with(|cs| KEPT.borrow_ref_mut(cs).take()) else {
        return;
    };
    kept.each(&mut each);
    critical_section::with(|cs| {
        *KEPT.borrow_ref_mut(cs) = Some(Kept {
            bytes: kept.bytes,
            len: 0,
        })
    });
}

pub fn take() -> Option<Line> {
    critical_section::with(|cs| QUEUE.borrow(cs).borrow_mut().pop())
}

pub fn metadata(recording: bool) {
    log::info!(target: "build", "commit={} dirty={} harness={} cycling={} recording={} level=INFO slots=8 bytes=384",
        option_env!("CYCLING_BUILD_COMMIT").unwrap_or("unknown"),
        option_env!("CYCLING_BUILD_DIRTY").unwrap_or("unknown"),
        cfg!(feature="debug-harness"), cfg!(feature="cycling"), recording);
}

pub fn stats() -> (u32, u32, u32, u32, usize, usize) {
    let (lost, depth) = critical_section::with(|cs| {
        let q = QUEUE.borrow_ref(cs);
        (q.lost(), q.depth())
    });
    (
        ENQUEUES.load(Ordering::Relaxed),
        ENQUEUE_US.load(Ordering::Relaxed),
        MAX_ENQUEUE_US.load(Ordering::Relaxed),
        lost,
        depth,
        core::mem::size_of_val(&QUEUE),
    )
}
