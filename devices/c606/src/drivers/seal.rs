//! The phone link's seal, protocol v2 (issue #545). The phone opens with
//! `nonce_p`, the device answers `nonce_d`, and both derive one key per
//! direction from the pairing key:
//! `HKDF-SHA256(ikm = K, salt = nonce_p ‖ nonce_d, info = "vana link 2")`
//! gives 64 bytes, phone-to-device first. Every later frame is
//! ChaCha20-Poly1305 under its direction's key, with that direction's frame
//! counter as the nonce (96 bits, little-endian, from 0) and no associated
//! data. Pure: the transport feeds bytes in and sends what comes out.
use chacha20poly1305::{AeadInPlace, ChaCha20Poly1305, KeyInit, Nonce, Tag};
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Bytes of each side's handshake nonce.
pub const HELLO: usize = 16;
/// Bytes of the tag that ends every sealed frame.
pub const TAG: usize = 16;
const INFO: &[u8] = b"vana link 2";

/// HKDF-SHA256 (RFC 5869) into `okm`, at most 255 hash lengths.
pub fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8], okm: &mut [u8]) {
    let mac = |key: &[u8]| {
        <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key).expect("HMAC takes any key")
    };
    let mut extract = mac(salt);
    extract.update(ikm);
    let prk = extract.finalize().into_bytes();
    let mut previous: &[u8] = &[];
    let mut block = [0u8; 32];
    for (index, chunk) in okm.chunks_mut(32).enumerate() {
        let mut expand = mac(&prk);
        expand.update(previous);
        expand.update(info);
        expand.update(&[index as u8 + 1]);
        block.copy_from_slice(&expand.finalize().into_bytes());
        chunk.copy_from_slice(&block[..chunk.len()]);
        previous = &block;
    }
}

/// The 96-bit nonce of frame `counter`.
pub fn nonce(counter: u64) -> [u8; 12] {
    let mut nonce = [0; 12];
    nonce[..8].copy_from_slice(&counter.to_le_bytes());
    nonce
}

/// One direction of an open link: its key and the next frame's counter.
pub struct Direction {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Direction {
    fn new(key: &[u8]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new_from_slice(key).expect("a 32-byte key"),
            counter: 0,
        }
    }

    /// Seal the plaintext `frame[..length]` in place and append its tag;
    /// `frame` holds at least `length + TAG` bytes. Returns the sealed length.
    pub fn seal(&mut self, frame: &mut [u8], length: usize) -> usize {
        let nonce = nonce(self.counter);
        self.counter += 1;
        let (text, rest) = frame.split_at_mut(length);
        let tag = self
            .cipher
            .encrypt_in_place_detached(Nonce::from_slice(&nonce), &[], text)
            .expect("a frame far below ChaCha20's limit");
        rest[..TAG].copy_from_slice(&tag);
        length + TAG
    }

    /// Open the sealed `frame` in place: the plaintext's length, at its
    /// front, or `None` when it does not authenticate.
    pub fn open(&mut self, frame: &mut [u8]) -> Option<usize> {
        let length = frame.len().checked_sub(TAG)?;
        let nonce = nonce(self.counter);
        let (text, tag) = frame.split_at_mut(length);
        self.cipher
            .decrypt_in_place_detached(Nonce::from_slice(&nonce), &[], text, Tag::from_slice(tag))
            .ok()?;
        self.counter += 1;
        Some(length)
    }

    #[cfg(test)]
    pub fn counter(&self) -> u64 {
        self.counter
    }
}

/// Both directions of a link, from the pairing key and the two hellos.
pub struct Link {
    /// Phone to device: what the device opens.
    pub incoming: Direction,
    /// Device to phone: what the device seals.
    pub outgoing: Direction,
}

impl Link {
    pub fn new(key: &[u8; 32], phone: &[u8; HELLO], device: &[u8; HELLO]) -> Self {
        let mut salt = [0; 2 * HELLO];
        salt[..HELLO].copy_from_slice(phone);
        salt[HELLO..].copy_from_slice(device);
        let mut keys = [0; 64];
        hkdf(key, &salt, INFO, &mut keys);
        Self {
            incoming: Direction::new(&keys[..32]),
            outgoing: Direction::new(&keys[32..]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> std::vec::Vec<u8> {
        let text: std::string::String = text.split_whitespace().collect();
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn hkdf_matches_rfc_5869_cases_1_and_3() {
        let mut okm = [0; 42];
        hkdf(
            &[0x0b; 22],
            &hex("000102030405060708090a0b0c"),
            &hex("f0f1f2f3f4f5f6f7f8f9"),
            &mut okm,
        );
        assert_eq!(
            okm[..],
            hex(
                "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf
                 34007208d5b887185865"
            )[..]
        );
        hkdf(&[0x0b; 22], &[], &[], &mut okm);
        assert_eq!(
            okm[..],
            hex(
                "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d
                 9d201395faa4b61a96c8"
            )[..]
        );
    }

    /// RFC 8439 section 2.8.2.
    #[test]
    fn seal_matches_rfc_8439_with_its_associated_data() {
        let key = hex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
        let nonce = hex("070000004041424344454647");
        let aad = hex("50515253c0c1c2c3c4c5c6c7");
        let mut text = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.".to_vec();
        let cipher = ChaCha20Poly1305::new_from_slice(&key).unwrap();
        let tag = cipher
            .encrypt_in_place_detached(Nonce::from_slice(&nonce), &aad, &mut text)
            .unwrap();
        assert_eq!(
            text[..16],
            hex("d31a8d34648e60db7b86afbc53ef7ec2")[..],
            "the ciphertext's first block"
        );
        assert_eq!(tag[..], hex("1ae10b594f09e26a7e902ecbd0600691")[..]);
    }

    #[test]
    fn the_nonce_is_the_counter_little_endian() {
        assert_eq!(nonce(0), [0; 12]);
        assert_eq!(nonce(0x0102), [2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn each_direction_opens_what_the_other_side_sealed_in_order_and_refuses_the_rest() {
        let key = [7; 32];
        let device = Link::new(&key, &[1; HELLO], &[2; HELLO]);
        let phone = Link::new(&key, &[1; HELLO], &[2; HELLO]);
        let (mut phone_out, mut phone_in) = (phone.incoming, phone.outgoing);
        let (mut device_in, mut device_out) = (device.incoming, device.outgoing);
        for round in 0..3u8 {
            let mut frame = [round; 40];
            let sealed = phone_out.seal(&mut frame, 24);
            assert_eq!(sealed, 24 + TAG);
            assert_eq!(device_in.open(&mut frame[..sealed]), Some(24));
            assert_eq!(frame[..24], [round; 24]);
        }
        let mut frame = [0; TAG];
        let sealed = device_out.seal(&mut frame, 0);
        assert_eq!(phone_in.open(&mut frame[..sealed]), Some(0));
        assert_eq!((device_in.counter(), device_out.counter()), (3, 1));

        let mut replayed = [9; 20 + TAG];
        let mut copy = replayed;
        phone_out.seal(&mut replayed, 20);
        copy.copy_from_slice(&replayed);
        assert_eq!(device_in.open(&mut replayed), Some(20));
        assert_eq!(device_in.open(&mut copy), None, "a replay is refused");
        let mut tampered = [9; 20 + TAG];
        phone_out.seal(&mut tampered, 20);
        tampered[3] ^= 1;
        assert_eq!(device_in.open(&mut tampered), None);
        assert_eq!(device_in.open(&mut [0; TAG - 1]), None);

        let other = Link::new(&[8; 32], &[1; HELLO], &[2; HELLO]);
        let mut frame = [5; 8 + TAG];
        let mut wrong = other.incoming;
        wrong.seal(&mut frame, 8);
        let mut fresh = Link::new(&key, &[1; HELLO], &[2; HELLO]).incoming;
        assert_eq!(fresh.open(&mut frame), None, "another key is refused");
    }
}
