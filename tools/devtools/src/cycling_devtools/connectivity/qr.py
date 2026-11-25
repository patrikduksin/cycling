"""A QR code encoder for short texts: byte mode, error correction level M, versions 1 to 10.

Enough for `cycling pair` to show its pairing URL in a terminal. The layout
follows ISO/IEC 18004; the mask is chosen by the standard penalty rules.
"""

# Level M per version 1..10: error correction codewords per block, and block count.
ECC_PER_BLOCK = (10, 16, 26, 18, 24, 16, 18, 22, 22, 26)
BLOCKS = (1, 1, 1, 2, 2, 4, 4, 4, 5, 5)
ALIGNMENT = ((), (6, 18), (6, 22), (6, 26), (6, 30), (6, 34), (6, 22, 38), (6, 24, 42), (6, 26, 46), (6, 28, 50))
MAX_VERSION = 10
FORMAT_M = 0b00

MASKS = (
    lambda x, y: (x + y) % 2 == 0,
    lambda x, y: y % 2 == 0,
    lambda x, y: x % 3 == 0,
    lambda x, y: (x + y) % 3 == 0,
    lambda x, y: (x // 3 + y // 2) % 2 == 0,
    lambda x, y: x * y % 2 + x * y % 3 == 0,
    lambda x, y: (x * y % 2 + x * y % 3) % 2 == 0,
    lambda x, y: ((x + y) % 2 + x * y % 3) % 2 == 0,
)


def raw_modules(version):
    """Modules left for data and error correction once the function patterns are drawn."""
    result = (16 * version + 128) * version + 64
    if version >= 2:
        count = version // 7 + 2
        result -= (25 * count - 10) * count - 55
        if version >= 7:
            result -= 36
    return result


def data_codewords(version):
    return raw_modules(version) // 8 - ECC_PER_BLOCK[version - 1] * BLOCKS[version - 1]


def gf_multiply(x, y):
    z = 0
    for i in reversed(range(8)):
        z = (z << 1) ^ ((z >> 7) * 0x11D)
        z ^= ((y >> i) & 1) * x
    return z


def rs_divisor(degree):
    result = [0] * (degree - 1) + [1]
    root = 1
    for _ in range(degree):
        for j in range(degree):
            result[j] = gf_multiply(result[j], root)
            if j + 1 < degree:
                result[j] ^= result[j + 1]
        root = gf_multiply(root, 0x02)
    return result


def rs_remainder(data, divisor):
    result = [0] * len(divisor)
    for byte in data:
        factor = byte ^ result.pop(0)
        result.append(0)
        for i, coefficient in enumerate(divisor):
            result[i] ^= gf_multiply(coefficient, factor)
    return result


def choose_version(length):
    for version in range(1, MAX_VERSION + 1):
        count_bits = 8 if version < 10 else 16
        if 4 + count_bits + 8 * length <= data_codewords(version) * 8:
            return version
    raise ValueError(f'{length} bytes do not fit a version {MAX_VERSION} QR code at level M')


def codewords(data, version):
    """Data with its padding, split into blocks, error correction added, interleaved."""
    capacity = data_codewords(version)
    count_bits = 8 if version < 10 else 16
    bits = '0100' + format(len(data), f'0{count_bits}b') + ''.join(format(byte, '08b') for byte in data)
    bits += '0' * min(4, capacity * 8 - len(bits))
    bits += '0' * (-len(bits) % 8)
    stream = [int(bits[at : at + 8], 2) for at in range(0, len(bits), 8)]
    stream += [(0xEC, 0x11)[i % 2] for i in range(capacity - len(stream))]

    blocks_count, ecc = BLOCKS[version - 1], ECC_PER_BLOCK[version - 1]
    short_count = blocks_count - raw_modules(version) // 8 % blocks_count
    short_data = raw_modules(version) // 8 // blocks_count - ecc
    divisor = rs_divisor(ecc)
    blocks, at = [], 0
    for i in range(blocks_count):
        length = short_data + (i >= short_count)
        block = stream[at : at + length]
        at += length
        blocks.append((block, rs_remainder(block, divisor)))
    result = [block[i] for i in range(short_data + 1) for block, _ in blocks if i < len(block)]
    return result + [parity[i] for i in range(ecc) for _, parity in blocks]


class Matrix:
    def __init__(self, version):
        self.version = version
        self.size = version * 4 + 17
        self.dark = [[False] * self.size for _ in range(self.size)]
        self.function = [[False] * self.size for _ in range(self.size)]

    def set(self, x, y, dark):
        self.dark[y][x] = dark
        self.function[y][x] = True

    def draw_function_patterns(self):
        size = self.size
        for i in range(size):
            self.set(6, i, i % 2 == 0)
            self.set(i, 6, i % 2 == 0)
        for cx, cy in ((3, 3), (size - 4, 3), (3, size - 4)):
            for dy in range(-4, 5):
                for dx in range(-4, 5):
                    x, y = cx + dx, cy + dy
                    if 0 <= x < size and 0 <= y < size:
                        self.set(x, y, max(abs(dx), abs(dy)) not in (2, 4))
        positions = ALIGNMENT[self.version - 1]
        last = len(positions) - 1
        for i, cx in enumerate(positions):
            for j, cy in enumerate(positions):
                if (i, j) in ((0, 0), (0, last), (last, 0)):
                    continue
                for dy in range(-2, 3):
                    for dx in range(-2, 3):
                        self.set(cx + dx, cy + dy, max(abs(dx), abs(dy)) != 1)
        self.draw_format(0)
        self.draw_version()

    def draw_format(self, mask):
        data = FORMAT_M << 3 | mask
        remainder = data
        for _ in range(10):
            remainder = (remainder << 1) ^ ((remainder >> 9) * 0x537)
        bits = (data << 10 | remainder) ^ 0x5412

        def bit(i):
            return (bits >> i) & 1 == 1

        size = self.size
        for i in range(6):
            self.set(8, i, bit(i))
        self.set(8, 7, bit(6))
        self.set(8, 8, bit(7))
        self.set(7, 8, bit(8))
        for i in range(9, 15):
            self.set(14 - i, 8, bit(i))
        for i in range(8):
            self.set(size - 1 - i, 8, bit(i))
        for i in range(8, 15):
            self.set(8, size - 15 + i, bit(i))
        self.set(8, size - 8, True)

    def draw_version(self):
        if self.version < 7:
            return
        remainder = self.version
        for _ in range(12):
            remainder = (remainder << 1) ^ ((remainder >> 11) * 0x1F25)
        bits = self.version << 12 | remainder
        for i in range(18):
            dark = (bits >> i) & 1 == 1
            a, b = self.size - 11 + i % 3, i // 3
            self.set(a, b, dark)
            self.set(b, a, dark)

    def place(self, data):
        size = self.size
        at = 0
        right = size - 1
        while right >= 1:
            if right == 6:
                right = 5
            for vertical in range(size):
                for j in range(2):
                    x = right - j
                    upward = (right + 1) & 2 == 0
                    y = size - 1 - vertical if upward else vertical
                    if not self.function[y][x] and at < len(data) * 8:
                        self.dark[y][x] = (data[at >> 3] >> (7 - (at & 7))) & 1 == 1
                        at += 1
            right -= 2

    def masked(self, mask):
        result = [row[:] for row in self.dark]
        test = MASKS[mask]
        for y in range(self.size):
            for x in range(self.size):
                if not self.function[y][x] and test(x, y):
                    result[y][x] = not result[y][x]
        return result


def finder_like(runs, n):
    """Patterns 1:1:3:1:1 (dark first) with four light modules on either side, in the last seven runs."""
    core = n > 0 and runs[2] == runs[4] == runs[5] == n and runs[3] == 3 * n
    return (core and runs[0] >= 4 * n and runs[6] >= n) + (core and runs[6] >= 4 * n and runs[0] >= n)


def line_penalty(line):
    """Rules 1 and 3 for one row or column; the border beyond it counts as light."""
    size = len(line)
    score = 0
    runs = [0] * 7

    def push(length):
        runs.insert(0, length + (size if runs[0] == 0 else 0))
        del runs[7:]

    color, run = False, 0
    for module in line:
        if module == color:
            run += 1
            score += 3 if run == 5 else run > 5
            continue
        push(run)
        if not color:
            score += 40 * finder_like(runs, runs[1])
        color, run = module, 1
    if color:
        push(run)
        run = 0
    push(run + size)
    return score + 40 * finder_like(runs, runs[1])


def penalty(dark):
    size = len(dark)
    score = sum(map(line_penalty, dark)) + sum(line_penalty(list(column)) for column in zip(*dark, strict=True))
    for y in range(size - 1):
        for x in range(size - 1):
            if dark[y][x] == dark[y][x + 1] == dark[y + 1][x] == dark[y + 1][x + 1]:
                score += 3
    total = size * size
    darks = sum(map(sum, dark))
    score += (-(-abs(darks * 20 - total * 10) // total) - 1) * 10
    return score


def encode(text, mask=None):
    """The module matrix (rows of booleans, True dark) of `text`; the mask is chosen when None."""
    data = text.encode() if isinstance(text, str) else bytes(text)
    version = choose_version(len(data))
    matrix = Matrix(version)
    matrix.draw_function_patterns()
    matrix.place(codewords(data, version))
    candidates = range(8) if mask is None else (mask,)
    best = None
    for candidate in candidates:
        matrix.draw_format(candidate)
        modules = matrix.masked(candidate)
        score = penalty(modules)
        if best is None or score < best[0]:
            best = (score, modules)
    return best[1]


def render(dark, invert=False, quiet=4):
    """Two module rows per text line in half blocks, with a quiet zone.

    A filled cell shows the terminal's text colour. On a dark terminal that colour
    is light, so light modules are filled; --invert fills dark modules instead for
    a light terminal.
    """
    size = len(dark)
    width = size + 2 * quiet
    rows = [[False] * width for _ in range(quiet)]
    rows += [[False] * quiet + list(row) + [False] * quiet for row in dark]
    rows += [[False] * width for _ in range(quiet + (len(rows) + quiet) % 2)]
    glyphs = {(True, True): '█', (True, False): '▀', (False, True): '▄', (False, False): ' '}
    lines = []
    for top, bottom in zip(rows[::2], rows[1::2], strict=True):
        lines.append(''.join(glyphs[(a == invert, b == invert)] for a, b in zip(top, bottom, strict=True)))
    return '\n'.join(lines)
