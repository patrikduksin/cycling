import os
import unittest

from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305

from cycling_devtools.connectivity.vana_link import (
    AuthenticationError,
    Link,
    Pairing,
    hkdf_sha256,
    open_sealed,
    parse_pairing,
    seal,
)

KEY = bytes(range(32))
URL = 'vanaride://pair?id=0a1b2c3d&k=AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8'


class AeadTest(unittest.TestCase):
    def test_rfc_8439_section_2_8_2(self):
        key = bytes(range(0x80, 0xA0))
        nonce = bytes.fromhex('070000004041424344454647')
        aad = bytes.fromhex('50515253c0c1c2c3c4c5c6c7')
        plaintext = (
            b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, "
            b'sunscreen would be it.'
        )
        expected = bytes.fromhex(
            'd31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b'
            '1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc'
            '3ff4def08e4b7a9de576d26586cec64b6116'
            '1ae10b594f09e26a7e902ecbd0600691'
        )
        self.assertEqual(seal(key, nonce, plaintext, aad), expected)
        self.assertEqual(open_sealed(key, nonce, expected, aad), plaintext)

    def test_random_seals_match_cryptography(self):
        for length in (0, 1, 15, 16, 17, 63, 64, 65, 1008, 3000):
            key, nonce, plaintext = os.urandom(32), os.urandom(12), os.urandom(length)
            sealed = seal(key, nonce, plaintext)
            self.assertEqual(sealed, ChaCha20Poly1305(key).encrypt(nonce, plaintext, None))
            self.assertEqual(open_sealed(key, nonce, sealed), plaintext)

    def test_a_changed_byte_fails_to_authenticate(self):
        sealed = bytearray(seal(KEY, bytes(12), b'GET /api/status'))
        for at in (0, len(sealed) - 1):
            tampered = bytearray(sealed)
            tampered[at] ^= 1
            with self.assertRaises(AuthenticationError):
                open_sealed(KEY, bytes(12), bytes(tampered))
        with self.assertRaises(AuthenticationError):
            open_sealed(os.urandom(32), bytes(12), bytes(sealed))


class HkdfTest(unittest.TestCase):
    def test_rfc_5869_case_1(self):
        okm = hkdf_sha256(b'\x0b' * 22, bytes(range(13)), bytes(range(0xF0, 0xFA)), 42)
        self.assertEqual(
            okm.hex(), '3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865'
        )


class LinkTest(unittest.TestCase):
    def test_phone_and_device_keys_mirror_and_counters_rise(self):
        nonce_p, nonce_d = os.urandom(16), os.urandom(16)
        phone = Link.derive(KEY, nonce_p, nonce_d)
        device = Link.derive(KEY, nonce_p, nonce_d, phone=False)
        okm = hkdf_sha256(KEY, nonce_p + nonce_d, b'vana link 2', 64)
        self.assertEqual((phone.send.key, phone.receive.key), (okm[:32], okm[32:]))
        for text in (b'one', b'two'):
            wire = phone.send.seal(text)
            self.assertEqual(int.from_bytes(wire[:2], 'little'), len(text) + 16)
            self.assertEqual(device.receive.open(wire[2:]), text)
        self.assertEqual((phone.send.counter, device.receive.counter), (2, 2))
        replayed = phone.send.seal(b'three')
        device.receive.counter = 0
        with self.assertRaises(AuthenticationError):
            device.receive.open(replayed[2:])


class PairingUrlTest(unittest.TestCase):
    def test_url_round_trip(self):
        pairing = parse_pairing(URL + '\n')
        self.assertEqual(pairing, Pairing(bytes.fromhex('0a1b2c3d'), KEY))
        self.assertEqual(pairing.url(), URL)

    def test_malformed_urls_are_refused(self):
        for url in (
            URL.replace('vanaride', 'https'),
            URL.replace('0a1b2c3d', '0A1B2C3D'),
            URL.replace('id=0a1b2c3d', 'id=0a1b2c'),
            URL[:-1],
            URL + '=',
            URL + '&x=1',
        ):
            with self.subTest(url=url), self.assertRaises(ValueError):
                parse_pairing(url)


if __name__ == '__main__':
    unittest.main()
