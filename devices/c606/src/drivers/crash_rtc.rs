//! RTC-fast panic marker and reset-reason adapter for the C606.

use core::ptr::addr_of;
use core::ptr::addr_of_mut;
use core::ptr::read_volatile;
use core::ptr::write_volatile;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering;

use device_api::crash::Kind;
use device_api::crash::Marker;
use device_api::crash::Reset;
use esp_hal::rtc_cntl::SocResetReason;

#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut PANIC_RECORD: [u32; device_api::crash::WORDS] = [0; device_api::crash::WORDS];

static CONTROLLED: AtomicBool = AtomicBool::new(false);
static HOOK_ENTERED: AtomicBool = AtomicBool::new(false);

pub fn take() -> Marker {
    let mut words = [0; device_api::crash::WORDS];
    let source = addr_of!(PANIC_RECORD).cast::<u32>();
    for (index, word) in words.iter_mut().enumerate() {
        *word = unsafe { read_volatile(source.add(index)) };
    }
    unsafe {
        let target = addr_of_mut!(PANIC_RECORD).cast::<u32>();
        write_volatile(target.add(device_api::crash::WORDS - 1), 0);
        for index in 0..device_api::crash::WORDS - 1 {
            write_volatile(target.add(index), 0);
        }
    }
    device_api::crash::inspect(&words)
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

#[unsafe(export_name = "custom_pre_backtrace")]
pub extern "Rust" fn custom_pre_backtrace() {
    if HOOK_ENTERED.swap(true, Ordering::AcqRel) {
        return;
    }
    let kind = if CONTROLLED.load(Ordering::Acquire) {
        Kind::Controlled
    } else {
        Kind::Panic
    };
    let words = device_api::crash::encode(kind, env!("CARGO_PKG_VERSION"));
    let target = addr_of_mut!(PANIC_RECORD).cast::<u32>();
    unsafe { write_volatile(target.add(device_api::crash::WORDS - 1), 0) };
    for (index, word) in words[..device_api::crash::WORDS - 1].iter().enumerate() {
        unsafe { write_volatile(target.add(index), *word) };
    }
    unsafe {
        write_volatile(
            target.add(device_api::crash::WORDS - 1),
            words[device_api::crash::WORDS - 1],
        )
    };
}

#[unsafe(export_name = "custom_halt")]
pub extern "Rust" fn custom_halt() -> ! {
    esp_hal::system::software_reset()
}
