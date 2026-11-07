//! Application-only N22 update protocol recovered from stock N21 1.956.
//! No addresses are sent: the retained vendor bootloader owns flash geometry.
//! The caller retains the complete image and exclusive UART ownership to completion.

use super::companion::crc16;

pub const CHUNK: usize = 128;
pub const MAX_FRAME: usize = CHUNK + 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Ready,
    Enter,
    Info,
    Data(u16),
    Verify,
    Complete,
    Failed,
}

pub struct Session {
    pub phase: Phase,
    pub retries: u8,
    size: usize,
    waiting_since: Option<u64>,
}

pub struct Packet {
    bytes: [u8; MAX_FRAME],
    len: usize,
}

impl Packet {
    pub fn identity() -> Self {
        let mut packet = Self {
            bytes: [0; MAX_FRAME],
            len: 16,
        };
        packet.bytes[..16].copy_from_slice(&super::companion::command(1, [1, 0, 0, 0, 0, 0, 0, 0]));
        packet
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Session {
    pub const fn new() -> Self {
        Self {
            phase: Phase::Ready,
            retries: 0,
            size: 0,
            waiting_since: None,
        }
    }

    pub fn start(&mut self, size: usize) -> bool {
        if self.phase != Phase::Ready || !(8..=128 * usize::from(u16::MAX)).contains(&size) {
            return false;
        }
        self.size = size;
        self.advance(Phase::Enter);
        true
    }

    pub fn count(&self) -> u16 {
        self.size.div_ceil(CHUNK) as u16
    }

    fn advance(&mut self, phase: Phase) {
        self.phase = phase;
        self.retries = 0;
        self.waiting_since = None;
    }

    /// Call only once the previous packet has been fully accepted by the UART.
    /// Each retry gets its full deadline, unlike stock's truncated final retry.
    pub fn due(&mut self, now: u64) -> bool {
        if matches!(self.phase, Phase::Ready | Phase::Complete | Phase::Failed) {
            return false;
        }
        let timeout = if self.phase == Phase::Info {
            5000
        } else {
            3000
        };
        if let Some(sent) = self.waiting_since {
            if now.saturating_sub(sent) < timeout {
                return false;
            }
            let limit = if self.phase == Phase::Verify { 30 } else { 5 };
            if self.retries >= limit {
                self.phase = Phase::Failed;
                return false;
            }
            self.retries += 1;
        }
        self.waiting_since = Some(now);
        true
    }

    pub fn fail(&mut self) {
        self.phase = Phase::Failed;
    }

    /// Invalid, short, unrelated and stale replies never advance the transfer.
    pub fn reply(&mut self, frame: &[u8]) {
        if self.waiting_since.is_none()
            || frame.len() < 8
            || !(1..=5).contains(&frame[4])
            || frame[5] != 1
        {
            return;
        }
        let p = &frame[6..frame.len() - 2];
        match self.phase {
            Phase::Enter if p.len() >= 8 && p[0] == 0x24 && p[1] == 0 && p[4] == 0 => {
                self.advance(Phase::Info);
            }
            Phase::Info
                if p.len() >= 11
                    && p[0] == 0x25
                    && p[1] == 2
                    && p[3..7] == (self.size as u32).to_le_bytes()
                    && p[7..9] == self.count().to_le_bytes() =>
            {
                self.advance(Phase::Data(0));
            }
            Phase::Data(index)
                if p.len() >= 6 && p[0] == 0x26 && p[5] == 0 && p[3..5] == index.to_le_bytes() =>
            {
                self.advance(if index + 1 == self.count() {
                    Phase::Verify
                } else {
                    Phase::Data(index + 1)
                });
            }
            Phase::Verify if p.len() >= 8 && p[0] == 0x24 && p[1] == 1 && p[5] == 1 => {
                self.advance(Phase::Complete);
            }
            _ => {}
        }
    }

    pub fn packet(&self, image: &[u8]) -> Option<Packet> {
        if image.len() != self.size {
            return None;
        }
        let mut packet = Packet {
            bytes: [0; MAX_FRAME],
            len: 0,
        };
        let p = &mut packet.bytes[6..];
        let n = match self.phase {
            Phase::Enter => {
                p[..8].copy_from_slice(&[0x24, 0, 0xff, 0, 0, 0xff, 0, 0xff]);
                8
            }
            Phase::Info => {
                p[..3].copy_from_slice(&[0x25, 2, 0]);
                p[3..7].copy_from_slice(&(self.size as u32).to_le_bytes());
                p[7..9].copy_from_slice(&self.count().to_le_bytes());
                11
            }
            Phase::Data(index) => {
                let offset = usize::from(index) * CHUNK;
                let n = CHUNK.min(image.len().checked_sub(offset)?);
                p[0] = 0x26;
                p[1..3].copy_from_slice(&((n + 5) as u16).to_le_bytes());
                p[3..5].copy_from_slice(&index.to_le_bytes());
                p[5..5 + n].copy_from_slice(&image[offset..offset + n]);
                5 + n
            }
            Phase::Verify => {
                p[..8].copy_from_slice(&[0x24, 1, 0xff, 0, 0, 0xff, 0xff, 0xff]);
                8
            }
            _ => return None,
        };
        packet.bytes[..6].copy_from_slice(&[0xa5, (n + 4) as u8, 0x6f, 0xf1, 2, 1]);
        let crc = crc16(&packet.bytes[..n + 6]);
        packet.bytes[n + 6..n + 8].copy_from_slice(&crc.to_le_bytes());
        packet.len = n + 8;
        Some(packet)
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// How long a custom identity is kept before the guard restores stock without a
/// host. Long enough for a USB observer to reconnect after the companion's own
/// startup power-cycles the ESP32, which wipes this RAM record.
const SOAK_FALLBACK_MS: u64 = 300_000;

/// How long a fresh boot waits for any identity before restoring stock. Stock
/// 1.902 itself first answers about 4.9 s after the ESP32 boots, and the first
/// boot after an update can be slower; 5 s restored stock over a patched image
/// that may simply have been late.
const PROBE_DEADLINE_MS: u64 = 30_000;

/// Fixed, stock-derived marker trial. This is a best-effort ESP-side restore,
/// not an nRF bootloader rollback: a dead application may ignore DFU entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialPhase {
    Probing,
    Ready,
    Uploading,
    Soak,
    Restoring,
    Restored,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialAction {
    Probe,
    Restore,
}

pub struct Trial {
    pub phase: TrialPhase,
    pub custom_seen: bool,
    /// Main uptime of the first custom identity answer in this boot.
    pub custom_at: Option<u64>,
    pub stock_seen: bool,
    began: Option<u64>,
    deadline: u64,
    probe_sent: Option<u64>,
    last_probe: Option<u64>,
}

impl Trial {
    pub const fn new() -> Self {
        Self {
            phase: TrialPhase::Probing,
            custom_seen: false,
            custom_at: None,
            stock_seen: false,
            began: None,
            deadline: 0,
            probe_sent: None,
            last_probe: None,
        }
    }

    pub fn exclusive(&self) -> bool {
        !matches!(self.phase, TrialPhase::Ready | TrialPhase::Restored)
    }

    pub fn start(&mut self) -> bool {
        if self.phase != TrialPhase::Ready || !self.stock_seen {
            return false;
        }
        self.phase = TrialPhase::Uploading;
        self.probe_sent = None;
        true
    }

    /// The host has recorded the custom identity: restore stock on the next tick
    /// instead of waiting out the fallback.
    pub fn acknowledge(&mut self) -> bool {
        if self.phase != TrialPhase::Soak || !self.custom_seen {
            return false;
        }
        self.deadline = 0;
        true
    }

    pub fn probe_sent(&mut self, now: u64) {
        self.probe_sent = Some(now);
    }

    /// Only consume a checksum-validated identity following our own query.
    pub fn reply(&mut self, frame: &[u8], now: u64) {
        if !matches!(
            self.phase,
            TrialPhase::Probing | TrialPhase::Soak | TrialPhase::Restoring
        ) || !self
            .probe_sent
            .is_some_and(|at| now >= at && now - at <= 2000)
            || frame.len() != 18
            || frame[4..8] != [5, 1, 1, 1]
        {
            return;
        }
        let version = &frame[14..16];
        if version == [2, 0xa3] {
            self.custom_seen = true;
            self.custom_at.get_or_insert(now);
            if self.phase == TrialPhase::Probing {
                self.phase = TrialPhase::Soak;
                self.deadline = now.saturating_add(SOAK_FALLBACK_MS);
            }
        } else if version == [2, 0x13] {
            self.stock_seen = true;
            if self.phase == TrialPhase::Probing {
                self.phase = TrialPhase::Ready;
            } else if self.phase == TrialPhase::Restoring {
                self.phase = TrialPhase::Restored;
            }
        }
        self.probe_sent = None;
    }

    pub fn tick(&mut self, now: u64, transfer: Phase) -> Option<TrialAction> {
        let began = *self.began.get_or_insert(now);
        match self.phase {
            TrialPhase::Probing if now.saturating_sub(began) >= PROBE_DEADLINE_MS => {
                // After any main power cycle this build independently checks the
                // companion again. Missing identity also gets one stock attempt.
                self.phase = TrialPhase::Restoring;
                self.probe_sent = None;
                return Some(TrialAction::Restore);
            }
            TrialPhase::Uploading => match transfer {
                Phase::Complete => {
                    self.phase = TrialPhase::Soak;
                    self.deadline = now.saturating_add(SOAK_FALLBACK_MS);
                    self.last_probe = None;
                }
                Phase::Failed => {
                    self.phase = TrialPhase::Restoring;
                    self.probe_sent = None;
                    return Some(TrialAction::Restore);
                }
                _ => return None,
            },
            TrialPhase::Soak if now >= self.deadline => {
                self.phase = TrialPhase::Restoring;
                self.probe_sent = None;
                return Some(TrialAction::Restore);
            }
            TrialPhase::Restoring => match transfer {
                Phase::Failed => {
                    self.phase = TrialPhase::Failed;
                    return None;
                }
                Phase::Complete => {
                    // Verification is read-only and bounded; do not loop DFU.
                    if self.deadline == 0 {
                        self.deadline = now.saturating_add(10_000);
                        self.last_probe = None;
                    }
                    if now >= self.deadline {
                        self.phase = TrialPhase::Failed;
                        return None;
                    }
                }
                _ => {
                    self.deadline = 0;
                    return None;
                }
            },
            TrialPhase::Ready | TrialPhase::Restored | TrialPhase::Failed => return None,
            _ => {}
        }
        if self
            .last_probe
            .is_none_or(|at| now.saturating_sub(at) >= 1000)
        {
            self.last_probe = Some(now);
            Some(TrialAction::Probe)
        } else {
            None
        }
    }
}

impl Default for Trial {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn reply(payload: &[u8]) -> Vec<u8> {
        let mut frame = std::vec![0xa5, (payload.len() + 4) as u8, 0x6f, 0xf1, 5, 1];
        frame.extend_from_slice(payload);
        frame.extend_from_slice(&crc16(&frame).to_le_bytes());
        frame
    }

    #[test]
    fn retries_are_bounded_and_last_retry_has_a_full_deadline() {
        let mut s = Session::new();
        assert!(!s.start(0));
        assert!(s.start(129));
        assert!(s.due(0));
        assert!(!s.due(2999));
        for i in 1..=5 {
            assert!(s.due(i * 3000));
        }
        assert!(!s.due(17999));
        assert_eq!(s.phase, Phase::Enter);
        assert!(!s.due(18000));
        assert_eq!(s.phase, Phase::Failed);
        assert!(!s.start(129));
    }

    #[test]
    fn only_matching_acknowledgments_advance_and_final_chunk_is_unpadded() {
        let mut s = Session::new();
        let image = [0x55; 129];
        assert!(s.start(image.len()));
        let enter = reply(&[0x24, 0, 0, 0, 0, 0, 0, 0]);
        s.reply(&enter); // No request sent yet.
        assert_eq!(s.phase, Phase::Enter);
        assert!(s.due(0));
        s.reply(&enter);
        assert_eq!(s.phase, Phase::Info);
        assert!(s.due(10));
        s.reply(&reply(&[0x25, 2, 0, 128, 0, 0, 0, 2, 0, 0, 0]));
        assert_eq!(s.phase, Phase::Info);
        s.reply(&reply(&[0x25, 2, 0, 129, 0, 0, 0, 2, 0, 0, 0]));
        assert_eq!(s.phase, Phase::Data(0));
        assert_eq!(s.packet(&image).unwrap().as_bytes().len(), 141);
        assert!(s.due(20));
        for payload in [
            &[0x26, 0, 0][..],
            &[0x26, 0, 0, 1, 0, 0],
            &[0x26, 0, 0, 0, 0, 1],
        ] {
            s.reply(&reply(payload));
            assert_eq!(s.phase, Phase::Data(0));
        }
        s.reply(&reply(&[0x26, 0, 0, 0, 0, 0]));
        assert_eq!(s.phase, Phase::Data(1));
        let last = s.packet(&image).unwrap();
        assert_eq!(last.as_bytes().len(), 14);
        assert_eq!(&last.as_bytes()[6..12], &[0x26, 6, 0, 1, 0, 0x55]);
        assert!(s.due(30));
        s.reply(&reply(&[0x26, 0, 0, 0, 0, 0])); // Duplicate old ACK.
        assert_eq!(s.phase, Phase::Data(1));
        s.reply(&reply(&[0x26, 0, 0, 1, 0, 0]));
        assert_eq!(s.phase, Phase::Verify);
        assert!(s.due(40));
        s.reply(&reply(&[0x24, 1, 0xff, 0, 0, 0, 0xff, 0xff])); // Still in bootloader.
        assert_eq!(s.phase, Phase::Verify);
        s.reply(&reply(&[0x24, 1, 0xff, 0, 0, 1, 0xff, 0xff]));
        assert_eq!(s.phase, Phase::Complete);
        assert!(!s.due(100000));
    }

    fn identity(minor: u8) -> Vec<u8> {
        reply(&[1, 1, 0, 0, 0, 0, 0, 0, 2, minor])
    }

    #[test]
    fn companion_trial_requires_fresh_identity_and_restores_after_the_fallback() {
        let mut t = Trial::new();
        assert!(t.exclusive());
        assert!(!t.start());
        t.reply(&identity(0x13), 1); // No query was sent.
        assert_eq!(t.phase, TrialPhase::Probing);
        assert_eq!(t.tick(0, Phase::Ready), Some(TrialAction::Probe));
        t.probe_sent(0);
        t.reply(&identity(0x13), 2001); // Expired query.
        assert_eq!(t.phase, TrialPhase::Probing);
        t.probe_sent(2010);
        t.reply(&identity(0x13), 2020);
        assert_eq!(t.phase, TrialPhase::Ready);
        assert!(!t.exclusive());
        assert!(t.start());
        assert!(!t.start());
        assert_eq!(t.tick(3000, Phase::Data(3)), None);
        assert_eq!(t.tick(20_000, Phase::Complete), Some(TrialAction::Probe));
        t.probe_sent(20_000);
        t.reply(&identity(0xa3), 20_010);
        assert!(t.custom_seen);
        assert_eq!(t.custom_at, Some(20_010));
        t.tick(319_999, Phase::Complete);
        assert_eq!(t.phase, TrialPhase::Soak);
        assert_eq!(t.tick(320_000, Phase::Complete), Some(TrialAction::Restore));
        assert_eq!(t.tick(320_001, Phase::Enter), None);
        assert_eq!(t.tick(340_000, Phase::Complete), Some(TrialAction::Probe));
        t.probe_sent(340_000);
        t.reply(&identity(0x13), 340_010);
        assert_eq!(t.phase, TrialPhase::Restored);
        assert!(!t.exclusive());
        assert_eq!(t.tick(1_000_000, Phase::Complete), None);
    }

    #[test]
    fn companion_trial_rearms_after_main_power_cycle_and_missing_identity_gets_one_restore() {
        let mut rebooted = Trial::new();
        assert_eq!(rebooted.tick(100, Phase::Ready), Some(TrialAction::Probe));
        rebooted.probe_sent(100);
        rebooted.reply(&identity(0xa3), 110);
        assert!(rebooted.custom_seen);
        assert_eq!(rebooted.phase, TrialPhase::Soak);
        assert_eq!(
            rebooted.tick(300_110, Phase::Ready),
            Some(TrialAction::Restore)
        );
        assert_eq!(rebooted.tick(300_111, Phase::Enter), None);
        assert_eq!(rebooted.tick(310_000, Phase::Failed), None);
        assert_eq!(rebooted.phase, TrialPhase::Failed);
        assert_eq!(rebooted.tick(1_000_000, Phase::Failed), None);

        let mut missing = Trial::new();
        assert_eq!(missing.tick(1, Phase::Ready), Some(TrialAction::Probe));
        missing.tick(10_000, Phase::Ready); // Later than stock's own first answer.
        missing.tick(30_000, Phase::Ready);
        assert_eq!(missing.phase, TrialPhase::Probing);
        assert_eq!(missing.tick(30_001, Phase::Ready), Some(TrialAction::Restore));
        assert_eq!(missing.tick(30_002, Phase::Enter), None);
        assert_eq!(
            missing.tick(60_000, Phase::Complete),
            Some(TrialAction::Probe)
        );
        assert_eq!(missing.tick(70_000, Phase::Complete), None);
        assert_eq!(missing.phase, TrialPhase::Failed);
    }

    #[test]
    fn companion_trial_host_acknowledgment_restores_only_an_observed_custom_image() {
        let mut t = Trial::new();
        assert!(!t.acknowledge());
        assert_eq!(t.tick(0, Phase::Ready), Some(TrialAction::Probe));
        t.probe_sent(0);
        t.reply(&identity(0x13), 10);
        assert!(!t.acknowledge()); // Stock: nothing to restore.

        let mut rebooted = Trial::new();
        assert_eq!(rebooted.tick(0, Phase::Ready), Some(TrialAction::Probe));
        rebooted.probe_sent(0);
        rebooted.reply(&identity(0xa3), 10);
        assert_eq!(rebooted.tick(5_000, Phase::Ready), Some(TrialAction::Probe));
        assert!(rebooted.acknowledge());
        assert_eq!(rebooted.tick(5_001, Phase::Ready), Some(TrialAction::Restore));
        assert!(!rebooted.acknowledge());
    }

    #[test]
    fn companion_trial_failed_upload_selects_stock_without_retrying_the_trial() {
        let mut t = Trial::new();
        t.probe_sent(0);
        t.reply(&identity(0x13), 1);
        assert!(t.start());
        assert_eq!(t.tick(2000, Phase::Failed), Some(TrialAction::Restore));
        assert_eq!(t.tick(2001, Phase::Enter), None);
        assert_eq!(t.phase, TrialPhase::Restoring);
        assert!(!t.start());
    }

    #[test]
    #[ignore = "requires the private stock image and independently recovered frame list"]
    fn stock_vectors() {
        let image = std::fs::read(std::env::var("CYCLING_COMPANION_DECODED").unwrap()).unwrap();
        let reference =
            std::fs::read_to_string(std::env::var("CYCLING_COMPANION_VECTORS").unwrap()).unwrap();
        let frames: Vec<Vec<u8>> = reference
            .lines()
            .map(|line| {
                let hex = line.split_whitespace().nth(2).unwrap();
                (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                    .collect()
            })
            .collect();
        let mut s = Session::new();
        assert!(s.start(image.len()));
        let phases = [Phase::Enter, Phase::Info]
            .into_iter()
            .chain((0..s.count()).map(Phase::Data))
            .chain([Phase::Verify]);
        let mut checked = 0;
        for (phase, expected) in phases.zip(&frames) {
            s.phase = phase;
            let actual = s.packet(&image).unwrap();
            assert_eq!(actual.as_bytes(), expected, "phase={phase:?}");
            checked += 1;
        }
        assert_eq!(checked, 617);
        assert_eq!(frames.len(), checked + 1); // Stock's final power cut is deliberately omitted.
        std::println!("Matched all {checked} update frames, including every image byte.");
    }
}
