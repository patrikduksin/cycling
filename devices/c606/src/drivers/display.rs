//! ST7789 transport on the C606's recovered 16-bit I80 wiring.
use device_api::display::Display as DisplayCapability;
use device_api::display::Geometry;
use device_api::observation::Error;
use esp_hal::Blocking;
use esp_hal::delay::Delay;
use esp_hal::dma::DmaTxBuf;
use esp_hal::lcd_cam::lcd::i8080::I8080;
use esp_hal::lcd_cam::lcd::i8080::I8080Transfer;

const PANEL_WIDTH: usize = 240;
const PANEL_HEIGHT: usize = 320;
/// Eight physical rows per DMA transfer; unchanged strip timing.
const STRIP_ROWS: usize = 8;
pub const STRIP_BYTES: usize = PANEL_WIDTH * STRIP_ROWS * 2;

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

    /// Render each strip into `next` while the previous strip transfers, then
    /// address and send it. Returns the bus and both buffers in every case.
    fn frame(
        bus: Bus<'d>,
        buffer: DmaTxBuf,
        mut next: DmaTxBuf,
        row: &mut impl FnMut(usize, &mut [u16]),
    ) -> (Result<(), Error>, Bus<'d>, DmaTxBuf, DmaTxBuf) {
        let mut line = [0u16; PANEL_WIDTH];
        let mut link = Link::Idle(bus, buffer);
        for top in (0..PANEL_HEIGHT).step_by(STRIP_ROWS) {
            next.set_length(STRIP_BYTES);
            let bytes = next.as_mut_slice();
            let (strip, _) = bytes[..STRIP_BYTES].as_chunks_mut::<{ PANEL_WIDTH * 2 }>();
            for (index, strip_row) in strip.iter_mut().enumerate() {
                row(top + index, &mut line);
                let (pixels, _) = strip_row.as_chunks_mut::<2>();
                for (pixel, value) in pixels.iter_mut().zip(line) {
                    *pixel = value.to_le_bytes();
                }
            }
            let (result, bus, free) = link.settle();
            if result.is_err() {
                return (result, bus, free, next);
            }
            let (result, bus, free) =
                Self::command(bus, free, 0x2a, &[0, 0, 0, (PANEL_WIDTH - 1) as u8]);
            if result.is_err() {
                return (result, bus, free, next);
            }
            let bottom = top + STRIP_ROWS - 1;
            let window = [
                (top >> 8) as u8,
                top as u8,
                (bottom >> 8) as u8,
                bottom as u8,
            ];
            let (result, bus, free) = Self::command(bus, free, 0x2b, &window);
            if result.is_err() {
                return (result, bus, free, next);
            }
            match bus.send(0x2c_u16, 0, next) {
                Ok(transfer) => {
                    link = Link::Busy(transfer);
                    next = free;
                }
                Err((_, bus, strip)) => return (Err(Error::Failed), bus, free, strip),
            }
        }
        let (result, bus, buffer) = link.settle();
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
        let _access = crate::capabilities::power::ACCESS.enter()?;
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
