//! Cycling speed and training-zone policy.
pub const WHEEL_MM: u32 = 2136;
#[derive(Default)]
pub struct Speed {
    previous: Option<(u16, u16, u64)>,
    epoch: Option<(u32, u32)>,
    value: Option<u32>,
    received: u64,
}
impl Speed {
    pub fn receive(&mut self, packet: device_api::ant::Packet) {
        let epoch = (packet.generation, packet.loss_count);
        if self.epoch != Some(epoch) {
            *self = Self::default();
            self.epoch = Some(epoch);
        }
        let data = packet.data;
        let time = u16::from_le_bytes([data[4], data[5]]);
        let rev = u16::from_le_bytes([data[6], data[7]]);
        let now = packet.received_ms;
        self.received = now;
        if let Some((old_time, old_rev, at)) = self.previous {
            let dt = time.wrapping_sub(old_time);
            let dr = rev.wrapping_sub(old_rev);
            if now.saturating_sub(at) > 60_000 {
                self.value = None;
            } else if dt != 0 && dr != 0 {
                let speed = u64::from(dr) * u64::from(WHEEL_MM) * 1024 / u64::from(dt);
                self.value = (speed <= 40_000).then_some(speed as u32);
            } else {
                return;
            }
        }
        self.previous = Some((time, rev, now));
    }
    pub fn value(&self, now: u64) -> Option<u32> {
        if now.saturating_sub(self.received) > 3000 {
            return None;
        }
        self.previous.and_then(|(_, _, at)| {
            if now.saturating_sub(at) > 4000 {
                Some(0)
            } else {
                self.value
            }
        })
    }
}

pub fn power_zone(w: u16) -> u8 {
    match w {
        0..=118 => 1,
        119..=161 => 2,
        162..=194 => 3,
        195..=226 => 4,
        227..=258 => 5,
        259..=323 => 6,
        _ => 7,
    }
}
pub fn heart_zone(b: u16) -> u8 {
    match b {
        0..=138 => 1,
        139..=153 => 2,
        154..=171 => 3,
        172..=184 => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speed_wrap_stop_and_loss() {
        fn packet(time: u16, rev: u16, at: u64, loss: u32) -> device_api::ant::Packet {
            let mut data = [0; 8];
            data[4..6].copy_from_slice(&time.to_le_bytes());
            data[6..8].copy_from_slice(&rev.to_le_bytes());
            device_api::ant::Packet {
                identity: device_api::ant::Identity {
                    device_type: 123,
                    device_number: 1,
                    transmission_type: 1,
                },
                data,
                received_ms: at,
                generation: 1,
                loss_count: loss,
            }
        }
        let mut speed = Speed::default();
        speed.receive(packet(65000, 65535, 100, 0));
        assert_eq!(speed.value(100), None);
        speed.receive(packet(488, 0, 1100, 0));
        assert_eq!(speed.value(1100), Some(WHEEL_MM));
        speed.receive(packet(488, 0, 5200, 0));
        assert_eq!(speed.value(5200), Some(0));
        assert_eq!(speed.value(8300), None);
        speed.receive(packet(1512, 1, 8500, 1));
        assert_eq!(speed.value(8500), None);
    }
    #[test]
    fn zones_cover_boundaries() {
        for (v, z) in [
            (118, 1),
            (119, 2),
            (161, 2),
            (162, 3),
            (194, 3),
            (195, 4),
            (226, 4),
            (227, 5),
            (258, 5),
            (259, 6),
            (323, 6),
            (324, 7),
        ] {
            assert_eq!(power_zone(v), z);
        }
        for (v, z) in [
            (138, 1),
            (139, 2),
            (153, 2),
            (154, 3),
            (171, 3),
            (172, 4),
            (184, 4),
            (185, 5),
        ] {
            assert_eq!(heart_zone(v), z);
        }
    }
}
