"""The VANA link (protocol v2): pairing URL, handshake and frame AEAD, standard library only.

ble-http runs under the system Python, so ChaCha20-Poly1305 (RFC 8439) and
HKDF-SHA256 (RFC 5869) are written out here. After the L2CAP channel opens the
phone sends a plaintext `u16 LE 16` frame holding nonce_p and the device answers
one holding nonce_d. Both derive HKDF-SHA256(K, salt=nonce_p||nonce_d,
info="vana link 2") into 64 bytes: k_pd (phone to device), then k_dp. Every
later frame is `u16 LE length` + ChaCha20-Poly1305 ciphertext and tag, its nonce
the direction's frame counter as 12 bytes little-endian, from 0.
"""

import base64
import hashlib
import hmac
import re
import struct
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from cycling_devtools.workspace import DEVICE_LOCAL

PAIRING_FILE = DEVICE_LOCAL / 'pairing' / 'vana.url'

INFO = b'vana link 2'
NONCE_BYTES = 16
KEY_BYTES = 32
ID_BYTES = 4
TAG_BYTES = 16
MASK = 0xFFFFFFFF


class AuthenticationError(ValueError):
    """A frame whose tag does not match: the peer holds another key, or the bytes changed."""


def _rotl(value, shift):
    return ((value << shift) & MASK) | (value >> (32 - shift))


def _quarter(s, a, b, c, d):
    s[a] = (s[a] + s[b]) & MASK
    s[d] = _rotl(s[d] ^ s[a], 16)
    s[c] = (s[c] + s[d]) & MASK
    s[b] = _rotl(s[b] ^ s[c], 12)
    s[a] = (s[a] + s[b]) & MASK
    s[d] = _rotl(s[d] ^ s[a], 8)
    s[c] = (s[c] + s[d]) & MASK
    s[b] = _rotl(s[b] ^ s[c], 7)


def chacha20_block(key, counter, nonce):
    state = [0x61707865, 0x3320646E, 0x79622D32, 0x6B206574]
    state = [*state, *struct.unpack('<8I', key), counter, *struct.unpack('<3I', nonce)]
    s = list(state)
    for _ in range(10):
        _quarter(s, 0, 4, 8, 12)
        _quarter(s, 1, 5, 9, 13)
        _quarter(s, 2, 6, 10, 14)
        _quarter(s, 3, 7, 11, 15)
        _quarter(s, 0, 5, 10, 15)
        _quarter(s, 1, 6, 11, 12)
        _quarter(s, 2, 7, 8, 13)
        _quarter(s, 3, 4, 9, 14)
    return struct.pack('<16I', *((x + y) & MASK for x, y in zip(s, state, strict=True)))


def chacha20(key, counter, nonce, data):
    stream = b''.join(chacha20_block(key, counter + i, nonce) for i in range((len(data) + 63) // 64))
    return (int.from_bytes(data, 'little') ^ int.from_bytes(stream[: len(data)], 'little')).to_bytes(
        len(data), 'little'
    )


def poly1305(key, message):
    r = int.from_bytes(key[:16], 'little') & 0x0FFFFFFC0FFFFFFC0FFFFFFC0FFFFFFF
    s = int.from_bytes(key[16:], 'little')
    p = (1 << 130) - 5
    acc = 0
    for at in range(0, len(message), 16):
        block = message[at : at + 16] + b'\x01'
        acc = (acc + int.from_bytes(block, 'little')) * r % p
    return ((acc + s) & ((1 << 128) - 1)).to_bytes(16, 'little')


def _tag(key, nonce, aad, ciphertext):
    mac_key = chacha20_block(key, 0, nonce)[:32]

    def padded(data):
        return data + bytes(-len(data) % 16)

    lengths = struct.pack('<QQ', len(aad), len(ciphertext))
    return poly1305(mac_key, padded(aad) + padded(ciphertext) + lengths)


def seal(key, nonce, plaintext, aad=b''):
    """ChaCha20-Poly1305 (RFC 8439 section 2.8): ciphertext followed by its 16-byte tag."""
    ciphertext = chacha20(key, 1, nonce, plaintext)
    return ciphertext + _tag(key, nonce, aad, ciphertext)


def open_sealed(key, nonce, sealed, aad=b''):
    if len(sealed) < TAG_BYTES:
        raise AuthenticationError('frame shorter than its tag')
    ciphertext, tag = sealed[:-TAG_BYTES], sealed[-TAG_BYTES:]
    if not hmac.compare_digest(tag, _tag(key, nonce, aad, ciphertext)):
        raise AuthenticationError('frame failed to authenticate')
    return chacha20(key, 1, nonce, ciphertext)


def hkdf_sha256(ikm, salt, info, length):
    prk = hmac.new(salt, ikm, hashlib.sha256).digest()
    okm, block = b'', b''
    for i in range(1, -(-length // 32) + 1):
        block = hmac.new(prk, block + info + bytes([i]), hashlib.sha256).digest()
        okm += block
    return okm[:length]


@dataclass(frozen=True)
class Pairing:
    id: bytes
    key: bytes

    @property
    def id_hex(self):
        return self.id.hex()

    def url(self):
        k = base64.urlsafe_b64encode(self.key).rstrip(b'=').decode()
        return f'vanaride://pair?id={self.id_hex}&k={k}'


def parse_pairing(url):
    """`vanaride://pair?id=<8 lowercase hex>&k=<base64url K, no padding>`."""
    parts = urlsplit(url.strip())
    query = parse_qs(parts.query)
    if parts.scheme != 'vanaride' or parts.netloc != 'pair' or set(query) != {'id', 'k'}:
        raise ValueError(f'not a vanaride://pair URL: {url.strip()!r}')
    (id_hex,), (k,) = query['id'], query['k']
    if not re.fullmatch(r'[0-9a-f]{8}', id_hex) or not re.fullmatch(r'[A-Za-z0-9_-]{43}', k):
        raise ValueError('pairing URL needs id=<8 lowercase hex> and k=<43 base64url characters>')
    return Pairing(bytes.fromhex(id_hex), base64.urlsafe_b64decode(k + '='))


def load_pairing(source=None):
    """A vanaride URL, or the path of a file holding one; `cycling pair`'s file by default."""
    if source and source.strip().startswith('vanaride:'):
        return parse_pairing(source)
    path = Path(source) if source else PAIRING_FILE
    try:
        return parse_pairing(path.read_text())
    except FileNotFoundError:
        raise ValueError(f'no pairing at {path}; run cycling pair, or pass --pairing URL') from None


def counter_nonce(counter):
    return counter.to_bytes(12, 'little')


def frame(payload):
    return struct.pack('<H', len(payload)) + payload


def take_frame(stream):
    """The first `u16 LE length` frame of `stream`: (payload, rest), or None while incomplete."""
    if len(stream) < 2:
        return None
    (length,) = struct.unpack_from('<H', stream)
    if len(stream) < 2 + length:
        return None
    return bytes(stream[2 : 2 + length]), bytes(stream[2 + length :])


@dataclass
class Direction:
    key: bytes
    counter: int = 0

    def seal(self, plaintext):
        sealed = seal(self.key, counter_nonce(self.counter), plaintext)
        self.counter += 1
        return frame(sealed)

    def open(self, sealed):
        plaintext = open_sealed(self.key, counter_nonce(self.counter), sealed)
        self.counter += 1
        return plaintext


@dataclass
class Link:
    """One side of an open session: frames it sends and frames it receives."""

    send: Direction
    receive: Direction

    @classmethod
    def derive(cls, key, nonce_p, nonce_d, phone=True):
        okm = hkdf_sha256(key, nonce_p + nonce_d, INFO, 64)
        k_pd, k_dp = okm[:32], okm[32:]
        return cls(Direction(k_pd), Direction(k_dp)) if phone else cls(Direction(k_dp), Direction(k_pd))
