//! Pairing with the VANA iPhone app. Over USB, `PHONE PAIR` makes a fresh
//! 32-byte key and a 4-byte id from hardware entropy and replies
//! `vanaride://pair?id=<hex8>&k=<base64url key>`, which the phone opens from
//! its camera and keeps. The shell saves the pairing with its settings, in
//! place of the one before, so the phone before stops working: one phone at
//! a time. While a pairing is saved the device opens the Bluetooth session
//! itself whenever no ride records; without one it never advertises.
use super::{Mode, Runtime};
use core::fmt::Write;
use device_api::server::{AccessPoint, Pairing};
use firmware_services::why;
use firmware_shell::shell::Shell;

const SEED: usize = 36;
const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The saved pairing, and the entropy and new pairing `PHONE PAIR` uses.
#[derive(Default)]
pub(super) struct Pairings {
    saved: Option<Pairing>,
    /// Made by `PHONE PAIR`, until the shell saves it.
    fresh: Option<Pairing>,
    /// Hardware entropy for the next pairing, drawn on a service step since
    /// a command has no entropy source of its own.
    seed: Option<[u8; SEED]>,
}

impl Pairings {
    pub(super) fn saved(&self) -> Option<Pairing> {
        self.saved
    }
}

fn write_id(out: &mut impl Write, id: [u8; 4]) -> core::fmt::Result {
    id.iter().try_for_each(|byte| write!(out, "{byte:02x}"))
}

/// The pairing's URL, as the phone's camera opens it.
pub fn url(pairing: &Pairing) -> heapless::String<80> {
    let mut url = heapless::String::new();
    let _ = url.push_str("vanaride://pair?id=");
    let _ = write_id(&mut url, pairing.id);
    let _ = url.push_str("&k=");
    let mut bits = 0u32;
    let mut held = 0;
    for byte in pairing.key {
        bits = bits << 8 | u32::from(byte);
        held += 8;
        while held >= 6 {
            held -= 6;
            let _ = url.push(char::from(BASE64URL[(bits >> held) as usize & 63]));
        }
    }
    if held > 0 {
        let _ = url.push(char::from(BASE64URL[(bits << (6 - held)) as usize & 63]));
    }
    url
}

impl Runtime {
    /// Keep entropy for the next `PHONE PAIR` at hand.
    pub(super) fn draw_seed(&mut self, ap: &mut impl AccessPoint) {
        if self.phone.pairing.seed.is_some() {
            return;
        }
        let mut seed = [0; SEED];
        for word in seed.chunks_mut(4) {
            word.copy_from_slice(&ap.entropy().to_le_bytes());
        }
        self.phone.pairing.seed = Some(seed);
    }

    /// The device opens its Bluetooth session now: it is paired, nothing
    /// else holds the phone session and no ride records.
    pub(super) fn bluetooth_due(&self, now: u64) -> bool {
        let phone = &self.phone;
        phone.pairing.saved.is_some()
            && phone.session.is_none()
            && phone.wanted.is_none()
            && !phone.stopping
            && !phone.no_bluetooth
            && phone.work.is_some()
            && now >= phone.reopen_at
            && !self.ride.recording()
            && !self.recorder_busy()
    }

    /// `PHONE PAIR`: a new pairing, saved on the next pass, and its URL.
    pub(in crate::app) fn pair_command(&mut self, output: &mut impl Write) -> &'static str {
        let Some(seed) = self.phone.pairing.seed.take() else {
            return "BUSY";
        };
        let mut pairing = Pairing {
            id: [0; 4],
            key: [0; 32],
        };
        pairing.id.copy_from_slice(&seed[..4]);
        pairing.key.copy_from_slice(&seed[4..]);
        self.phone.pairing.fresh = Some(pairing);
        let written = write!(output, "id=")
            .and_then(|()| write_id(output, pairing.id))
            .and_then(|()| write!(output, " url={}", url(&pairing)));
        if written.is_err() {
            return "OUTPUT";
        }
        "ACCEPTED"
    }

    /// What `PHONE` reports of the pairing: the saved id, `pending` while a
    /// new one waits for its save, or `none`.
    pub(in crate::app) fn write_paired(&self, output: &mut impl Write) -> core::fmt::Result {
        match (self.phone.pairing.fresh, self.phone.pairing.saved) {
            (Some(_), _) => write!(output, " paired=pending"),
            (None, Some(saved)) => {
                write!(output, " paired=")?;
                write_id(output, saved.id)
            }
            (None, None) => write!(output, " paired=none"),
        }
    }

    /// Save a new pairing through the shell, once, and follow the pairing
    /// the shell holds.
    #[inline(never)]
    pub(in crate::app) fn save_pairing<D, I, P, B>(&mut self, shell: &mut Shell<D, I, P, B>)
    where
        D: device_api::display::Display,
        I: device_api::input::InputSource,
        P: device_api::power::Power,
        B: device_api::storage::OwnedFlash,
    {
        if let Some(fresh) = self.phone.pairing.fresh.take() {
            let saved = shell.save_pairing(fresh);
            why!("phone.paired", saved);
            log::info!(target: "phone", "VANA_PHONE paired saved={saved}");
        }
        self.follow_pairing(shell.settings.pairing);
    }

    /// Use the pairing the shell holds. A Bluetooth session of another
    /// pairing ends: its phone no longer opens the link.
    pub(in crate::app) fn follow_pairing(&mut self, pairing: Option<Pairing>) {
        let phone = &mut self.phone;
        if phone.pairing.saved != pairing
            && phone
                .session
                .as_ref()
                .is_some_and(|session| session.status.mode == Mode::Bluetooth)
        {
            phone.wanted = Some(None);
        }
        phone.pairing.saved = pairing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_carries_the_id_in_hex_and_the_key_in_unpadded_base64url() {
        let mut key = [0; 32];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(37) ^ 0xfb;
        }
        let url = url(&Pairing {
            id: [0x0a, 0xbc, 0x00, 0xff],
            key,
        });
        assert_eq!(
            url.as_str(),
            "vanaride://pair?id=0abc00ff&k=-96xlG9CJfjTtolsRxr90KuOYUQf8tWog2Y5HPfKrYA"
        );
        assert_eq!(url.len(), 30 + 43);
    }
}
