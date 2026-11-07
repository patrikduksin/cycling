# /// script
# dependencies = ["python-flint", "numpy"]
# ///
"""CRC-16 GCD search for header u16 @0x404 and @0x406, plus the residue scheme."""
import itertools, struct, sys, time
import numpy as np
from flint import nmod_poly
A = open(sys.argv[1], 'rb').read(); B = open(sys.argv[2], 'rb').read()
F = 0x404
def covs(img):
    out = {}
    for fill in (b'\0', b'\xff'):
        z = bytearray(img); z[F:F+4] = fill * 4; out['fill4' + fill.hex()] = bytes(z)
        z = bytearray(img); z[F:F+2] = fill * 2; out['fill2' + fill.hex()] = bytes(z)
    out.update(removed4=img[:F] + img[F+4:], removed2=img[:F] + img[F+2:], from408=img[F+4:], from406=img[F+2:],
               from480=img[0x480:], skipdesc=img[:0x400] + img[0x480:], from1000=img[0x1000:])
    return out
def poly(data, refl, init, value, w=16):
    n = len(data)
    bits = np.unpackbits(np.frombuffer(data, np.uint8), bitorder='little' if refl else 'big')
    c = np.zeros(8 * n + w, np.uint8); c[w:] = bits[::-1]
    c[8 * n:8 * n + w] ^= np.array([(init >> (w - 1 - k)) & 1 for k in range(w)], np.uint8)[::-1]
    c[:w] ^= np.array([(value >> k) & 1 for k in range(w)], np.uint8)
    return nmod_poly(c.tolist(), 2)
rev = lambda v: int(f'{v:016b}'[::-1], 2)
ca, cb = covs(A), covs(B)
t0 = time.time(); hits = 0
for cov, refl, init, xo, off, end in itertools.product(ca, (False, True), (0, 0xffff), (0, 0xffff), (4, 6), ('<H', '>H')):
    ta = struct.unpack_from(end, A, 0x400 + off)[0] ^ xo; tb = struct.unpack_from(end, B, 0x400 + off)[0] ^ xo
    if refl: ta, tb = rev(ta), rev(tb)
    g = poly(ca[cov], refl, init, ta).gcd(poly(cb[cov], refl, init, tb))
    if g.degree() >= 16:
        f16 = [f for f, e in g.factor()[1] if f.degree() == 16]
        print(f'deg={g.degree()} cov={cov} refl={refl} init={init:#x} xo={xo:#x} off={off} {end} deg16={f16}', flush=True); hits += 1
print(f'done {time.time()-t0:.0f}s hits={hits}')
