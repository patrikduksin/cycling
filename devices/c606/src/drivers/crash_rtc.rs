//! RTC-fast panic record, its flash copy, panic handler and reset-reason
//! adapter for the C606.
//!
//! The RTC record survives the software reset that ends a panic, and deep
//! sleep, but not power loss. Boot copies it to the `Region::Crash` journal
//! (`firmware_services::crash_log`), so `CRASH` still has it after the rider
//! switches off. See `device_api::crash` for what a record keeps.

use core::ptr::addr_of;
use core::ptr::addr_of_mut;
use core::ptr::read_volatile;
use core::ptr::write_volatile;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering;

use device_api::crash::Kind;
use device_api::crash::Marker;
use device_api::crash::Reset;
use device_api::storage::OwnedFlash;
use esp_hal::rtc_cntl::SocResetReason;
use firmware_services::crash_log;
use firmware_services::storage::RegionAccess;

#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut PANIC_RECORD: [u32; device_api::crash::WORDS] = [0; device_api::crash::WORDS];

static CONTROLLED: AtomicBool = AtomicBool::new(false);
/// Set by the first panic; a panic inside the handler resets at once.
static HOOK_ENTERED: AtomicBool = AtomicBool::new(false);

fn read() -> [u32; device_api::crash::WORDS] {
    let mut words = [0; device_api::crash::WORDS];
    let source = addr_of!(PANIC_RECORD).cast::<u32>();
    for (index, word) in words.iter_mut().enumerate() {
        // SAFETY: index < WORDS, so the pointer stays inside PANIC_RECORD. The
        // panic handler is the only other writer and resets when done.
        *word = unsafe { read_volatile(source.add(index)) };
    }
    words
}

/// What the last run left for this boot. A valid record is copied to flash
/// before it is marked reported; anything else is cleared. The copy writes
/// only when flash does not already hold the record, so it costs one sector
/// erase per panic, and a boot after a failed copy tries again.
pub fn take<B: OwnedFlash>(flash: &mut RegionAccess<'_, B>) -> Marker {
    let words = read();
    let marker = device_api::crash::inspect(&words);
    if device_api::crash::record(&words).is_some() {
        let mut reported = words;
        reported[device_api::crash::SEEN] = device_api::crash::REPORTED;
        let started = esp_hal::time::Instant::now();
        let kept = crash_log::keep(flash, &reported);
        if !matches!(kept, Ok(false)) {
            esp_println::println!(
                "CYCLING_CRASH_KEPT ok={} elapsed_us={}",
                kept.is_ok(),
                started.elapsed().as_micros()
            );
        }
        let target = addr_of_mut!(PANIC_RECORD).cast::<u32>();
        // SAFETY: SEEN is inside PANIC_RECORD. Boot runs this once before any
        // task or the panic handler can write the record; SEEN is outside the
        // checksum, so the record stays valid.
        unsafe {
            write_volatile(
                target.add(device_api::crash::SEEN),
                device_api::crash::REPORTED,
            )
        };
    } else {
        clear_rtc();
    }
    marker
}

/// Where `CRASH` found the record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Rtc,
    Flash,
}

impl Source {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rtc => "rtc",
            Self::Flash => "flash",
        }
    }
}

/// The last panic recorded, reported or not, until `clear` or the next panic.
/// A valid RTC record is the newest: flash holds only copies of RTC records,
/// and a later panic replaces the RTC record before boot copies it.
#[inline(never)]
pub fn last<B: OwnedFlash>(
    flash: &mut RegionAccess<'_, B>,
) -> Result<Option<(device_api::crash::Record, Source)>, B::Error> {
    if let Some(record) = device_api::crash::record(&read()) {
        return Ok(Some((record, Source::Rtc)));
    }
    Ok(crash_log::newest(flash)?
        .and_then(|entry| device_api::crash::record(&entry.words))
        .map(|record| (record, Source::Flash)))
}

/// Forget the recorded panic, in flash and then in RTC memory. A flash
/// failure, including closed storage in charging standby, keeps both, so the
/// record never resurfaces from flash after a clear that reported success.
pub fn clear<B: OwnedFlash>(flash: &mut RegionAccess<'_, B>) -> Result<(), B::Error> {
    crash_log::clear(flash)?;
    clear_rtc();
    Ok(())
}

fn clear_rtc() {
    let target = addr_of_mut!(PANIC_RECORD).cast::<u32>();
    // SAFETY: every offset is below WORDS, inside PANIC_RECORD. The validity
    // word is cleared first so a partial clear never looks valid; a panic
    // that interrupts the clear rewrites the whole record.
    unsafe {
        write_volatile(target.add(device_api::crash::VALID), 0);
        for index in 0..device_api::crash::VALID {
            write_volatile(target.add(index), 0);
        }
    }
}

pub fn reset() -> Reset {
    match esp_hal::system::reset_reason() {
        None => Reset::Unknown,
        Some(SocResetReason::ChipPowerOn) => Reset::Power,
        Some(SocResetReason::CoreSw | SocResetReason::CpuSw) => Reset::Software,
        Some(
            SocResetReason::CoreMwdt0
            | SocResetReason::CoreMwdt1
            | SocResetReason::CoreRtcWdt
            | SocResetReason::CpuMwdt0
            | SocResetReason::CpuMwdt1
            | SocResetReason::CpuRtcWdt
            | SocResetReason::SysRtcWdt
            | SocResetReason::SysSuperWdt,
        ) => Reset::Watchdog,
        Some(SocResetReason::SysBrownOut) => Reset::Brownout,
        Some(SocResetReason::CoreUsbUart | SocResetReason::CoreUsbJtag) => Reset::Usb,
        Some(_) => Reset::Other,
    }
}

#[cfg(feature = "debug-harness")]
pub fn controlled_panic() -> ! {
    CONTROLLED.store(true, Ordering::Release);
    panic!("controlled crash test")
}

/// Bytes of the message kept while formatting; the rest is dropped.
struct Message {
    bytes: [u8; device_api::crash::MESSAGE_BYTES],
    len: usize,
}

impl core::fmt::Write for Message {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let room = self.bytes.len() - self.len;
        let taken = text.len().min(room);
        self.bytes[self.len..self.len + taken].copy_from_slice(&text.as_bytes()[..taken]);
        self.len += taken;
        if taken < text.len() {
            Err(core::fmt::Error)
        } else {
            Ok(())
        }
    }
}

fn store(words: &[u32; device_api::crash::WORDS]) {
    let target = addr_of_mut!(PANIC_RECORD).cast::<u32>();
    // HOOK_ENTERED admits one panic-handler entry, the only runtime writer. The
    // validity word is cleared first and written last, so a reset mid-write
    // leaves an invalid rather than a torn record.
    // SAFETY: VALID is inside PANIC_RECORD; single writer as above.
    unsafe { write_volatile(target.add(device_api::crash::VALID), 0) };
    for (index, word) in words[..device_api::crash::VALID].iter().enumerate() {
        // SAFETY: index < VALID is inside PANIC_RECORD; single writer.
        unsafe { write_volatile(target.add(index), *word) };
    }
    // SAFETY: VALID is inside PANIC_RECORD; single writer.
    unsafe {
        write_volatile(
            target.add(device_api::crash::VALID),
            words[device_api::crash::VALID],
        )
    };
}

const fn is_one(value: Option<&str>) -> bool {
    matches!(value, Some(text) if text.len() == 1 && text.as_bytes()[0] == b'1')
}

/// How this image was built, for finding its symbols.
const BUILD: device_api::crash::Build = device_api::crash::Build {
    dirty: is_one(option_env!("CYCLING_BUILD_DIRTY")),
    // The SDK is application composition, which hardware code may not test
    // for; build.sh's CYCLING_SDK names it for the ELF archive just the same.
    cycling: is_one(option_env!("CYCLING_SDK")),
    harness: cfg!(feature = "debug-harness"),
    maintenance: cfg!(feature = "bulk-maintenance"),
};

/// Record where and why the firmware panicked, print it as esp-backtrace
/// did, so `mise run backtrace` still decodes captured logs, then reset.
///
/// The record is stored twice: first without the message, then with it, so
/// a panic while formatting the message (or a stack overflow that formatting
/// runs into) still leaves where it happened. A panic inside the handler
/// resets at once.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write as _;
    // No task switch or interrupt handler runs from here to the reset.
    // SAFETY: the matching release never runs; the reset ends the section.
    let _ = unsafe { critical_section::acquire() };
    if HOOK_ENTERED.swap(true, Ordering::AcqRel) {
        esp_hal::system::software_reset()
    }
    let backtrace = esp_backtrace::Backtrace::capture();
    let mut pcs = [0u32; device_api::crash::FRAMES];
    let mut frames = 0;
    for (slot, frame) in pcs.iter_mut().zip(backtrace.frames()) {
        *slot = frame.program_counter() as u32;
        frames += 1;
    }
    let kind = if CONTROLLED.load(Ordering::Acquire) {
        Kind::Controlled
    } else {
        Kind::Panic
    };
    let (file, line) = info
        .location()
        .map_or(("", 0), |location| (location.file(), location.line()));
    let mut panic = device_api::crash::Panic {
        kind,
        build: BUILD,
        version: env!("CARGO_PKG_VERSION"),
        commit: option_env!("CYCLING_BUILD_COMMIT").unwrap_or(""),
        uptime_ms: esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_millis() as u32,
        file,
        line,
        message: &[],
        pcs: &pcs[..frames],
    };
    store(&device_api::crash::encode(&panic));
    let mut message = Message {
        bytes: [0; device_api::crash::MESSAGE_BYTES],
        len: 0,
    };
    let _ = write!(message, "{}", info.message());
    panic.message = &message.bytes[..message.len];
    store(&device_api::crash::encode(&panic));
    esp_println::println!("");
    esp_println::println!("====================== PANIC ======================");
    esp_println::println!("{}", info);
    esp_println::println!("");
    esp_println::println!("Backtrace:");
    esp_println::println!("");
    for pc in &pcs[..frames] {
        esp_println::println!("0x{:x}", pc);
    }
    esp_hal::system::software_reset()
}
