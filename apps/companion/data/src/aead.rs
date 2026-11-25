//! ChaCha20-Poly1305 (RFC 8439) and HKDF-SHA256 (RFC 5869), written out
//! here: Exact Build compiles offline, with only the crates exact2's lock
//! holds, and that lock has SHA-256 but no AEAD. The tests hold each to its
//! RFC's vectors.

use sha2::{Digest, Sha256};

fn quarter(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// RFC 8439 §2.3.
fn block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    for i in 0..8 {
        state[4 + i] = le32(&key[4 * i..]);
    }
    state[12] = counter;
    for i in 0..3 {
        state[13 + i] = le32(&nonce[4 * i..]);
    }
    let mut s = state;
    for _ in 0..10 {
        quarter(&mut s, 0, 4, 8, 12);
        quarter(&mut s, 1, 5, 9, 13);
        quarter(&mut s, 2, 6, 10, 14);
        quarter(&mut s, 3, 7, 11, 15);
        quarter(&mut s, 0, 5, 10, 15);
        quarter(&mut s, 1, 6, 11, 12);
        quarter(&mut s, 2, 7, 8, 13);
        quarter(&mut s, 3, 4, 9, 14);
    }
    let mut out = [0; 64];
    for i in 0..16 {
        out[4 * i..4 * i + 4].copy_from_slice(&s[i].wrapping_add(state[i]).to_le_bytes());
    }
    out
}

/// RFC 8439 §2.4, from block 1 as the AEAD uses it.
fn chacha20(key: &[u8; 32], nonce: &[u8; 12], data: &mut [u8]) {
    for (i, chunk) in data.chunks_mut(64).enumerate() {
        let stream = block(key, 1 + i as u32, nonce);
        chunk.iter_mut().zip(stream).for_each(|(b, k)| *b ^= k);
    }
}

/// RFC 8439 §2.5, in 26-bit limbs.
fn poly1305(key: &[u8; 32], msg: &[u8]) -> [u8; 16] {
    const M: u32 = 0x3ff_ffff;
    let r = [
        le32(&key[0..]) & 0x3ff_ffff,
        (le32(&key[3..]) >> 2) & 0x3ff_ff03,
        (le32(&key[6..]) >> 4) & 0x3ff_c0ff,
        (le32(&key[9..]) >> 6) & 0x3f0_3fff,
        (le32(&key[12..]) >> 8) & 0x00f_ffff,
    ];
    let s = [r[1] * 5, r[2] * 5, r[3] * 5, r[4] * 5];
    let mut h = [0u32; 5];
    for chunk in msg.chunks(16) {
        let mut m = [0u8; 17];
        m[..chunk.len()].copy_from_slice(chunk);
        m[chunk.len()] = 1;
        h[0] += le32(&m[0..]) & M;
        h[1] += (le32(&m[3..]) >> 2) & M;
        h[2] += (le32(&m[6..]) >> 4) & M;
        h[3] += (le32(&m[9..]) >> 6) & M;
        h[4] += (le32(&m[12..]) >> 8) | (u32::from(m[16]) << 24);
        let w = |x: u32, y: u32| u64::from(x) * u64::from(y);
        let d = [
            w(h[0], r[0]) + w(h[1], s[3]) + w(h[2], s[2]) + w(h[3], s[1]) + w(h[4], s[0]),
            w(h[0], r[1]) + w(h[1], r[0]) + w(h[2], s[3]) + w(h[3], s[2]) + w(h[4], s[1]),
            w(h[0], r[2]) + w(h[1], r[1]) + w(h[2], r[0]) + w(h[3], s[3]) + w(h[4], s[2]),
            w(h[0], r[3]) + w(h[1], r[2]) + w(h[2], r[1]) + w(h[3], r[0]) + w(h[4], s[3]),
            w(h[0], r[4]) + w(h[1], r[3]) + w(h[2], r[2]) + w(h[3], r[1]) + w(h[4], r[0]),
        ];
        let mut carry = 0u64;
        for i in 0..5 {
            let v = d[i] + carry;
            h[i] = (v as u32) & M;
            carry = v >> 26;
        }
        h[0] += carry as u32 * 5;
        h[1] += h[0] >> 26;
        h[0] &= M;
    }
    let mut c;
    c = h[1] >> 26;
    h[1] &= M;
    for limb in &mut h[2..] {
        *limb += c;
        c = *limb >> 26;
        *limb &= M;
    }
    h[0] += c * 5;
    c = h[0] >> 26;
    h[0] &= M;
    h[1] += c;
    // h - p, kept when it does not go below zero.
    let mut g = [0u32; 5];
    c = 5;
    for i in 0..4 {
        g[i] = h[i] + c;
        c = g[i] >> 26;
        g[i] &= M;
    }
    g[4] = (h[4] + c).wrapping_sub(1 << 26);
    let keep_g = (g[4] >> 31).wrapping_sub(1);
    for i in 0..5 {
        h[i] = (h[i] & !keep_g) | (g[i] & keep_g);
    }
    let words = [h[0] | (h[1] << 26), (h[1] >> 6) | (h[2] << 20), (h[2] >> 12) | (h[3] << 14), (h[3] >> 18) | (h[4] << 8)];
    let mut tag = [0u8; 16];
    let mut f = 0u64;
    for i in 0..4 {
        f = u64::from(words[i]) + u64::from(le32(&key[16 + 4 * i..])) + (f >> 32);
        tag[4 * i..4 * i + 4].copy_from_slice(&(f as u32).to_le_bytes());
    }
    tag
}

fn tag(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], cipher: &[u8]) -> [u8; 16] {
    let otk: [u8; 32] = block(key, 0, nonce)[..32].try_into().expect("32");
    let pad = |n: usize| vec![0u8; (16 - n % 16) % 16];
    let data = [aad, &pad(aad.len()), cipher, &pad(cipher.len()), &(aad.len() as u64).to_le_bytes(), &(cipher.len() as u64).to_le_bytes()]
        .concat();
    poly1305(&otk, &data)
}

/// RFC 8439 §2.8: the ciphertext, then its 16-byte tag.
pub fn seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let mut out = plain.to_vec();
    chacha20(key, nonce, &mut out);
    let t = tag(key, nonce, aad, &out);
    out.extend_from_slice(&t);
    out
}

/// The plaintext, or `None` when the tag does not authenticate.
pub fn open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let (cipher, given) = sealed.split_at(sealed.len().checked_sub(16)?);
    let want = tag(key, nonce, aad, cipher);
    if want.iter().zip(given).fold(0, |d, (a, b)| d | (a ^ b)) != 0 {
        return None;
    }
    let mut plain = cipher.to_vec();
    chacha20(key, nonce, &mut plain);
    Some(plain)
}

fn hmac(key: &[u8], data: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    data.iter().for_each(|d| inner.update(d));
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// HKDF-SHA256 (RFC 5869): extract, then expand into `out`.
pub fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) {
    let prk = hmac(salt, &[ikm]);
    let mut t: Vec<u8> = Vec::new();
    for (i, chunk) in out.chunks_mut(32).enumerate() {
        t = hmac(&prk, &[&t, info, &[i as u8 + 1]]).to_vec();
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn key(s: &str) -> [u8; 32] {
        hex(s).try_into().unwrap()
    }

    #[test]
    fn hkdf_matches_rfc_5869_test_cases_1_and_3() {
        let mut okm = [0; 42];
        hkdf(&[0x0b; 22], &hex("000102030405060708090a0b0c"), &hex("f0f1f2f3f4f5f6f7f8f9"), &mut okm);
        assert_eq!(okm.to_vec(), hex("3cb25f25faacd57a90434f64d0362f2a 2d2d0a90cf1a5a4c5db02d56ecc4c5bf 34007208d5b887185865"));
        hkdf(&[0x0b; 22], &[], &[], &mut okm);
        assert_eq!(okm.to_vec(), hex("8da4e775a563c18f715f802a063c5a31 b8a11f5c5ee1879ec3454e5f3c738d2d 9d201395faa4b61a96c8"));
    }

    #[test]
    fn the_block_function_matches_rfc_8439_section_2_3_2() {
        let k = key("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let n: [u8; 12] = hex("000000090000004a00000000").try_into().unwrap();
        assert_eq!(
            block(&k, 1, &n).to_vec(),
            hex("10f1e7e4d13b5915500fdd1fa32071c4 c7d1f4c733c068030422aa9ac3d46c4e d2826446079faa0914c2d705d98b02a2
                 b5129cd1de164eb9cbd083e8a2503c4e")
        );
    }

    #[test]
    fn poly1305_matches_rfc_8439_section_2_5_2_and_its_edge_vectors() {
        let k = key("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b");
        assert_eq!(poly1305(&k, b"Cryptographic Forum Research Group").to_vec(), hex("a8061dc1305136c6c22b8baf0c0127a9"));
        let two = key("0200000000000000000000000000000000000000000000000000000000000000");
        let one = key("0100000000000000000000000000000000000000000000000000000000000000");
        // RFC 8439 A.3 vectors 5, 6, 8 and 9: the reductions modulo 2^130 - 5.
        assert_eq!(poly1305(&two, &[0xff; 16]).to_vec(), hex("03000000000000000000000000000000"));
        let s_ff = key("02000000000000000000000000000000ffffffffffffffffffffffffffffffff");
        assert_eq!(poly1305(&s_ff, &hex("02000000000000000000000000000000")).to_vec(), hex("03000000000000000000000000000000"));
        let eight = [vec![0xff; 16], [vec![0xfb], vec![0xfe; 15]].concat(), vec![0x01; 16]].concat();
        assert_eq!(poly1305(&one, &eight).to_vec(), vec![0; 16]);
        let nine = [vec![0xfd], vec![0xff; 15]].concat();
        assert_eq!(poly1305(&two, &nine).to_vec(), [vec![0xfa], vec![0xff; 15]].concat());
    }

    #[test]
    fn the_aead_matches_rfc_8439_section_2_8_2() {
        let k = key("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
        let nonce: [u8; 12] = hex("070000004041424344454647").try_into().unwrap();
        let aad = hex("50515253c0c1c2c3c4c5c6c7");
        let plain = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let sealed = seal(&k, &nonce, &aad, plain);
        assert_eq!(
            sealed,
            hex("d31a8d34648e60db7b86afbc53ef7ec2 a4aded51296e08fea9e2b5a736ee62d6 3dbea45e8ca9671282fafb69da92728b
                 1a71de0a9e060b2905d6a5b67ecd3b36 92ddbd7f2d778b8c9803aee328091b58 fab324e4fad675945585808b4831d7bc
                 3ff4def08e4b7a9de576d26586cec64b 6116 1ae10b594f09e26a7e902ecbd0600691")
        );
        assert_eq!(open(&k, &nonce, &aad, &sealed).as_deref(), Some(&plain[..]));
        let mut bad = sealed.clone();
        bad[0] ^= 1;
        assert_eq!(open(&k, &nonce, &aad, &bad), None);
        assert_eq!(open(&k, &nonce, &[], &sealed), None);
    }
}
