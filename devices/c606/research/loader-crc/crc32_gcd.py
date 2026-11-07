# /// script
# dependencies = ["python-flint", "numpy"]
# ///
"""Find a CRC-32 polynomial P with P | F(1.638) and P | F(1.902), F = M*x^32 + I*x^(8n) + V."""
import itertools, struct, sys, time
import numpy as np
from flint import nmod_poly

A = open(sys.argv[1], 'rb').read()  # N22 1.638 decoded image
B = open(sys.argv[2], 'rb').read()  # N22 1.902 decoded image
FIELD = 0x404

def coverages(img):
    z = bytearray(img); z[FIELD:FIELD+4] = b'\0' * 4
    f = bytearray(img); f[FIELD:FIELD+4] = b'\xff' * 4
    pad = lambda b: bytes(b) + b'\xff' * ((-len(b)) % 4096)
    return {
        'zeroed': bytes(z),
        'ffed': bytes(f),
        'removed': img[:FIELD] + img[FIELD+4:],
        'from408': img[FIELD+4:],
        'from480': img[0x480:],
        'skipdesc': img[:0x400] + img[0x480:],
        'zeroed_pad4k': pad(z),
        'before400': img[:0x400],
    }

def poly(data, refl, init, value):
    n = len(data)
    bits = np.unpackbits(np.frombuffer(data, np.uint8), bitorder='little' if refl else 'big')
    coeffs = np.zeros(8 * n + 32, np.uint8)
    coeffs[32:] = bits[::-1]                      # M(x) * x^32, first stream bit = top degree
    ib = np.array([(init >> (31 - k)) & 1 for k in range(32)], np.uint8)  # I(x) * x^(8n)
    coeffs[8 * n:8 * n + 32] ^= ib[::-1]
    vb = np.array([(value >> k) & 1 for k in range(32)], np.uint8)
    coeffs[:32] ^= vb
    return nmod_poly(coeffs.tolist(), 2)

def rev32(v):
    return int(f'{v:032b}'[::-1], 2)

def wordswap(b):
    b = b + b'\0' * ((-len(b)) % 4)
    a = np.frombuffer(b, '<u4').byteswap()
    return a.tobytes()

ca, cb = coverages(A), coverages(B)
va = {'le': struct.unpack_from('<I', A, FIELD)[0], 'be': struct.unpack_from('>I', A, FIELD)[0]}
vb = {'le': struct.unpack_from('<I', B, FIELD)[0], 'be': struct.unpack_from('>I', B, FIELD)[0]}
start = time.time(); count = 0
for cov, refl, ws, init, xo, end in itertools.product(ca, (False, True), (False, True), (0, 0xffffffff), (0, 0xffffffff), ('le', 'be')):
    da, db = ca[cov], cb[cov]
    if ws:
        da, db = wordswap(da), wordswap(db)
    # Non-reflected register value before the final XOR; reflected CRCs reverse the result.
    ta, tb = va[end] ^ xo, vb[end] ^ xo
    if refl:
        ta, tb = rev32(ta), rev32(tb)
    g = poly(da, refl, init, ta).gcd(poly(db, refl, init, tb))
    count += 1
    d = g.degree()
    if d >= 8:
        print(f'deg={d} cov={cov} refl={refl} wordswap={ws} init={init:#x} xorout={xo:#x} value={end}', flush=True)
        if d >= 32:
            for f, e in g.factor()[1]:
                print('   factor deg', f.degree(), f, '^', e, flush=True)
print(f'{count} variants in {time.time() - start:.1f}s')
