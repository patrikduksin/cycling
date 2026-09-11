"""Canvas comparison with explicit dynamic masks and RGB565 tolerances."""

PIXELS = 80 * 106

RIDE_STATUS_MASKS = ((48, 3, 32, 5),)


def read_rgb565(path):
    data = path.read_bytes()
    if len(data) != PIXELS * 2:
        raise ValueError(f'Expected {PIXELS * 2} raw bytes, got {len(data)}')
    return [int.from_bytes(data[i:i + 2], 'little') for i in range(0, len(data), 2)]


def masked(index, masks):
    x, y = index % 80, index // 80
    return any(left <= x < left + width and top <= y < top + height
               for left, top, width, height in masks)


def channels(pixel):
    return pixel >> 11, (pixel >> 5) & 63, pixel & 31


def compare(actual, expected, *, masks=(), tolerance=(0, 0, 0)):
    if len(actual) != PIXELS or len(expected) != PIXELS:
        raise ValueError('Canvas comparison requires two 80x106 images')
    mismatches = []
    for index, (got, wanted) in enumerate(zip(actual, expected, strict=True)):
        if masked(index, masks):
            continue
        if any(abs(a - b) > limit
               for a, b, limit in zip(channels(got), channels(wanted), tolerance, strict=True)):
            mismatches.append((index % 80, index // 80, got, wanted))
            if len(mismatches) == 10:
                break
    if mismatches:
        raise AssertionError(f'Canvas differs outside masks/tolerance: {mismatches}')


# The right-hand values and labels move when the preceding number changes width, so
# those row tails are masked. Fixed left labels are checked separately below.
DIAGNOSTICS_MASKS = (
    (15, 13, 65, 5),
    (20, 22, 60, 5),
    (39, 31, 41, 5),
    (55, 40, 25, 5),
    (30, 49, 50, 5),
    (3, 58, 60, 5),
    (15, 67, 65, 5),
    (23, 76, 57, 5),
    (19, 85, 20, 5),
    (57, 85, 20, 5),
    (7, 94, 4, 5),
    (17, 94, 4, 5),
)

DIAGNOSTICS_STATIC_REGIONS = (
    (3, 3, 44, 5),    # DIAGNOSTICS
    (3, 13, 8, 5),    # UP
    (3, 22, 16, 5),   # F MS
    (3, 31, 32, 5),   # HEAP KIB
    (3, 40, 48, 5),   # MIN SAMP KIB
    (3, 49, 24, 5),   # PS KIB
    (3, 67, 8, 5),    # OK
    (3, 76, 16, 5),   # UART
    (3, 85, 12, 5),   # RST
    (45, 85, 8, 5),   # CR
    (3, 94, 4, 5),    # H
    (13, 94, 4, 5),   # R
    (27, 94, 32, 5),  # TOP BACK
)


def compare_regions(actual, expected, regions):
    """Compare named static regions exactly, independent of dynamic masks."""
    mismatches = []
    for left, top, width, height in regions:
        for y in range(top, top + height):
            for x in range(left, left + width):
                index = y * 80 + x
                if actual[index] != expected[index]:
                    mismatches.append((x, y, actual[index], expected[index]))
                    if len(mismatches) == 10:
                        raise AssertionError(f'Static canvas labels differ: {mismatches}')
    if mismatches:
        raise AssertionError(f'Static canvas labels differ: {mismatches}')
