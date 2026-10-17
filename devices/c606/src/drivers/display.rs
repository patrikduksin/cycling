//! ST7789 transport on the C606's recovered 16-bit I80 wiring.
use core::sync::atomic::{AtomicU32, Ordering};
use device_api::display::Display as DisplayCapability;
use device_api::display::Geometry;
use device_api::observation::Error;
use embassy_time::Instant;
use esp_hal::Blocking;
use esp_hal::delay::Delay;
use esp_hal::dma::DmaTxBuf;
use esp_hal::lcd_cam::lcd::i8080::I8080;
use esp_hal::lcd_cam::lcd::i8080::I8080Transfer;

const PANEL_WIDTH: usize = 240;
const PANEL_HEIGHT: usize = 320;
/// Physical rows per DMA transfer. Fewer, larger strips cost fewer bus
/// commands and DMA restarts; two strips must still fit internal DMA RAM.
const STRIP_ROWS: usize = 16;
pub const STRIP_BYTES: usize = PANEL_WIDTH * STRIP_ROWS * 2;
const _: () = assert!(PANEL_HEIGHT % STRIP_ROWS == 0);

/// Fill a strip of `STRIP_ROWS` rows starting at panel row `top`. Rows render
/// in place when the DMA buffer is 2-byte aligned (it is word-aligned), else
/// through a line buffer.
fn render(bytes: &mut [u8], top: usize, row: &mut impl FnMut(usize, &mut [u16])) {
    // SAFETY: every bit pattern is a valid u16, and `align_to_mut` keeps the
    // middle slice inside `bytes` with u16 alignment.
    let (prefix, pixels, _) = unsafe { bytes.align_to_mut::<u16>() };
    if prefix.is_empty() {
        // Native little-endian u16s are the bytes the bus sends.
        for (index, line) in pixels.chunks_exact_mut(PANEL_WIDTH).enumerate() {
            row(top + index, line);
        }
        return;
    }
    let mut line = [0u16; PANEL_WIDTH];
    for (index, strip_row) in bytes.chunks_exact_mut(PANEL_WIDTH * 2).enumerate() {
        row(top + index, &mut line);
        for (pixel, value) in strip_row.chunks_exact_mut(2).zip(line) {
            pixel.copy_from_slice(&value.to_le_bytes());
        }
    }
}

/// Microseconds of the latest frame, of it spent waiting for the bus, and the
/// slowest frame since boot.
pub struct Timing {
    frame_us: AtomicU32,
    wait_us: AtomicU32,
    max_us: AtomicU32,
}
impl Timing {
    fn record(&self, frame_us: u64, wait_us: u64) {
        let frame_us = frame_us.min(u32::MAX as u64) as u32;
        self.frame_us.store(frame_us, Ordering::Relaxed);
        self.wait_us
            .store(wait_us.min(u32::MAX as u64) as u32, Ordering::Relaxed);
        self.max_us.fetch_max(frame_us, Ordering::Relaxed);
    }
    /// (frame, waiting for the bus, slowest frame), in microseconds.
    pub fn snapshot(&self) -> (u32, u32, u32) {
        (
            self.frame_us.load(Ordering::Relaxed),
            self.wait_us.load(Ordering::Relaxed),
            self.max_us.load(Ordering::Relaxed),
        )
    }
}
pub static TIMING: Timing = Timing {
    frame_us: AtomicU32::new(0),
    wait_us: AtomicU32::new(0),
    max_us: AtomicU32::new(0),
};

type Bus<'d> = I8080<'d, Blocking>;

/// The bus is idle with a free buffer, or busy sending a strip.
enum Link<'d> {
    Idle(Bus<'d>, DmaTxBuf),
    Busy(I8080Transfer<'d, DmaTxBuf, Blocking>),
}
impl<'d> Link<'d> {
    /// Wait for any strip in flight; the bus and its buffer always come back.
    fn settle(self) -> (Result<(), Error>, Bus<'d>, DmaTxBuf) {
        match self {
            Link::Idle(bus, buffer) => (Ok(()), bus, buffer),
            Link::Busy(transfer) => {
                let (result, bus, buffer) = transfer.wait();
                (result.map_err(|_| Error::Failed), bus, buffer)
            }
        }
    }
}

pub struct Display<'d> {
    resources: Option<(Bus<'d>, DmaTxBuf)>,
    /// A second strip buffer, so one strip renders while the previous one sends.
    spare: Option<DmaTxBuf>,
}

impl<'d> Display<'d> {
    /// Both buffers hold at least `STRIP_BYTES`.
    pub fn new(bus: Bus<'d>, buffer: DmaTxBuf, spare: DmaTxBuf) -> Self {
        Self {
            resources: Some((bus, buffer)),
            spare: Some(spare),
        }
    }

    /// A synchronous command with parameters; the bus and buffer always come back.
    fn command(
        bus: Bus<'d>,
        mut buffer: DmaTxBuf,
        command: u8,
        bytes: &[u8],
    ) -> (Result<(), Error>, Bus<'d>, DmaTxBuf) {
        buffer.fill(bytes);
        match bus.send(command, 0, buffer) {
            Ok(transfer) => Link::Busy(transfer).settle(),
            Err((_, bus, buffer)) => (Err(Error::Failed), bus, buffer),
        }
    }

    /// Render each strip straight into `next` while the previous strip
    /// transfers. The panel window covers all rows once, so the first strip
    /// starts a memory write (RAMWR) and every later one continues it
    /// (RAMWRC) with no per-strip addressing. Returns the bus and both
    /// buffers in every case.
    fn frame(
        bus: Bus<'d>,
        buffer: DmaTxBuf,
        mut next: DmaTxBuf,
        row: &mut impl FnMut(usize, &mut [u16]),
    ) -> (Result<(), Error>, Bus<'d>, DmaTxBuf, DmaTxBuf) {
        let started = Instant::now();
        let mut waited = 0;
        let (result, bus, buffer) =
            Self::command(bus, buffer, 0x2a, &[0, 0, 0, (PANEL_WIDTH - 1) as u8]);
        if result.is_err() {
            return (result, bus, buffer, next);
        }
        let bottom = PANEL_HEIGHT - 1;
        let window = [0, 0, (bottom >> 8) as u8, bottom as u8];
        let (result, bus, buffer) = Self::command(bus, buffer, 0x2b, &window);
        if result.is_err() {
            return (result, bus, buffer, next);
        }
        let mut link = Link::Idle(bus, buffer);
        for top in (0..PANEL_HEIGHT).step_by(STRIP_ROWS) {
            next.set_length(STRIP_BYTES);
            render(&mut next.as_mut_slice()[..STRIP_BYTES], top, row);
            let blocked = Instant::now();
            let (result, bus, free) = link.settle();
            waited += blocked.elapsed().as_micros();
            if result.is_err() {
                return (result, bus, free, next);
            }
            let command: u16 = if top == 0 { 0x2c } else { 0x3c };
            match bus.send(command, 0, next) {
                Ok(transfer) => {
                    link = Link::Busy(transfer);
                    next = free;
                }
                Err((_, bus, strip)) => return (Err(Error::Failed), bus, free, strip),
            }
        }
        let blocked = Instant::now();
        let (result, bus, buffer) = link.settle();
        waited += blocked.elapsed().as_micros();
        TIMING.record(started.elapsed().as_micros(), waited);
        (result, bus, buffer, next)
    }

    fn send(&mut self, command: u8, bytes: &[u8]) -> Result<(), Error> {
        // Every path below puts the bus and buffer back before returning, and
        // `send` is not re-entrant, so the resources are always present here.
        let (bus, buffer) = self
            .resources
            .take()
            .expect("display bus and buffer are returned after each transfer");
        let (result, bus, buffer) = Self::command(bus, buffer, command, bytes);
        self.resources = Some((bus, buffer));
        result
    }

    /// Boot-time panel bring-up. A bus failure here panics: there is no
    /// working display to degrade to this early in boot.
    pub fn init(&mut self) {
        let delay = Delay::new();
        self.send(0x01, &[])
            .expect("panel software reset at startup");
        delay.delay_millis(150);
        self.send(0x11, &[]).expect("panel sleep-out at startup");
        delay.delay_millis(120);
        // Controller configuration recovered from stock, validated by the C demo.
        const INIT: &[(u8, &[u8])] = &[
            (0x36, &[0]),
            (0x3a, &[0x55]),
            (0xb2, &[0x0c, 0x0c, 0, 0x33, 0x33]),
            (0xb7, &[0x74]),
            (0xbb, &[0x1e]),
            (0xc0, &[0x2c]),
            (0xc2, &[1]),
            (0xc3, &[0x10]),
            (0xc4, &[0x20]),
            (0xc6, &[0x0f]),
            (0xd0, &[0xa4, 0xa1]),
            (
                0xe0,
                &[
                    0xf0, 6, 0x0b, 6, 7, 0x25, 0x34, 0x44, 0x4a, 0x38, 0x14, 0x13, 0x2e, 0x34,
                ],
            ),
            (
                0xe1,
                &[
                    0xf0, 0x0c, 0x10, 0x0a, 9, 6, 0x33, 0x43, 0x49, 0x36, 0x12, 0x14, 0x2a, 0x32,
                ],
            ),
            (0xe9, &[0x11, 0x11, 3]),
            (0x21, &[]),
            (0x29, &[]),
        ];
        for &(command, data) in INIT {
            self.send(command, data)
                .expect("panel configuration at startup");
            delay.delay_millis(2);
        }
    }
}
impl DisplayCapability for Display<'_> {
    fn geometry(&self) -> Geometry {
        Geometry {
            width: PANEL_WIDTH,
            height: PANEL_HEIGHT,
        }
    }
    fn submit(&mut self, mut row: impl FnMut(usize, &mut [u16])) -> Result<(), Error> {
        // Charging standby closes work admission but keeps the panel open,
        // so the application can show the charge; nothing waits on it.
        let _access = match crate::capabilities::power::ACCESS.enter() {
            Ok(lease) => Some(lease),
            Err(_) if crate::capabilities::power::standby() => None,
            Err(error) => return Err(error),
        };
        // Every path in `frame` returns the bus and both buffers, and `submit`
        // is not re-entrant, so they are always present here.
        let (bus, buffer) = self
            .resources
            .take()
            .expect("display bus and buffer are returned after each transfer");
        let spare = self
            .spare
            .take()
            .expect("display strip buffer is returned after each frame");
        let (result, bus, buffer, spare) = Self::frame(bus, buffer, spare, &mut row);
        self.resources = Some((bus, buffer));
        self.spare = Some(spare);
        result
    }
}
