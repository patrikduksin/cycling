//! ST7789 transport on the C606's recovered 16-bit I80 wiring.
use cycling_os::{coin::PIXELS, screenshot::canvas_index};
use esp_hal::{Blocking, delay::Delay, dma::DmaTxBuf, lcd_cam::lcd::i8080::I8080};

pub struct Display<'d> {
    resources: Option<(I8080<'d, Blocking>, DmaTxBuf)>,
}

impl<'d> Display<'d> {
    pub fn new(bus: I8080<'d, Blocking>, buffer: DmaTxBuf) -> Self {
        Self {
            resources: Some((bus, buffer)),
        }
    }

    fn send(&mut self, command: u8, bytes: &[u8], pixels: bool) {
        let (bus, mut buffer) = self.resources.take().unwrap();
        buffer.fill(bytes);
        let transfer = if pixels {
            bus.send(command as u16, 0, buffer)
        } else {
            bus.send(command, 0, buffer)
        };
        let (result, bus, buffer) = match transfer {
            Ok(transfer) => transfer.wait(),
            Err((error, bus, buffer)) => (Err(error), bus, buffer),
        };
        self.resources = Some((bus, buffer));
        result.unwrap();
    }

    pub fn init(&mut self) {
        let delay = Delay::new();
        self.send(0x01, &[], false);
        delay.delay_millis(150);
        self.send(0x11, &[], false);
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
            self.send(command, data, false);
            delay.delay_millis(2);
        }
    }

    pub fn draw(&mut self, canvas: &[u16; PIXELS]) {
        // Eight physical rows per DMA transfer; nearest-neighbor 3× enlargement.
        let mut strip = [0u8; 240 * 8 * 2];
        for top in (0..320usize).step_by(8) {
            for row in 0..8 {
                for x in 0..240 {
                    let color = canvas[canvas_index(x, top + row)].to_le_bytes();
                    let offset = (row * 240 + x) * 2;
                    strip[offset..offset + 2].copy_from_slice(&color);
                }
            }
            self.send(0x2a, &[0, 0, 0, 239], false);
            self.send(
                0x2b,
                &[
                    (top >> 8) as u8,
                    top as u8,
                    ((top + 7) >> 8) as u8,
                    (top + 7) as u8,
                ],
                false,
            );
            self.send(0x2c, &strip, true);
        }
    }
}
