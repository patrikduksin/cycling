# /// script
# dependencies = ["python-flint", "numpy"]
# ///
"""Residue scheme: CRC over the whole image (field included) equals an unknown constant R.
Then P | D = F(1.638) - F(1.902) with V = 0; P's irreducible factors of degree d | 32 divide
gcd(x^(2^32) - x, D)."""
import itertools, sys, time
import numpy as np
from flint import nmod_poly

A = open(sys.argv[1], 'rb').read(); B = open(sys.argv[2], 'rb').read()

def poly(data, refl, init):
    n = len(data)
    bits = np.unpackbits(np.frombuffer(data, np.uint8), bitorder='little' if refl else 'big')
    c = np.zeros(8 * n + 32, np.uint8); c[32:] = bits[::-1]
    c[8 * n:8 * n + 32] ^= np.array([(init >> (31 - k)) & 1 for k in range(32)], np.uint8)[::-1]
    return c

def wordswap(b):
    return np.frombuffer(b + b'\0' * ((-len(b)) % 4), '<u4').byteswap().tobytes()

x = nmod_poly([0, 1], 2)
t0 = time.time()
for refl, ws, init in itertools.product((False, True), (False, True), (0, 0xffffffff)):
    da, db = (wordswap(A), wordswap(B)) if ws else (A, B)
    ca, cb = poly(da, refl, init), poly(db, refl, init)
    n = max(len(ca), len(cb)); c = np.zeros(n, np.uint8); c[:len(ca)] ^= ca; c[:len(cb)] ^= cb
    D = nmod_poly(c.tolist(), 2)
    # Strip factors x and x+1 (every candidate has a constant term; x+1 alone is parity).
    h = x
    for _ in range(32):
        h = (h * h) % D
    g = (h - x).gcd(D)
    fs = [(f.degree(), e) for f, e in g.factor()[1]] if g.degree() > 0 else []
    big = [f for f, e in (g.factor()[1] if g.degree() > 0 else []) if f.degree() >= 8]
    print(f'refl={refl} wordswap={ws} init={init:#x} gcd_deg={g.degree()} factor_degrees={sorted(fs)} {time.time()-t0:.0f}s', flush=True)
    for f in big:
        print('   ', f.degree(), f, flush=True)
