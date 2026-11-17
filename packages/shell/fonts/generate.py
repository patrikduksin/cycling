# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow==11.3.0"]
# ///
"""Rasterize the UI typefaces into 4-bit alpha glyphs for `rendering::font`.

Run `uv run packages/shell/fonts/generate.py` after changing a face below; it
rewrites `packages/shell/src/rendering/font/faces.rs`. `--scale 0.75` writes
`faces_0_75.rs`, every face at that scale step, pixel sizes rounded half up.
The source is Nunito, a rounded sans under the SIL Open Font License (see
OFL.txt beside this file).
"""

import argparse
from dataclasses import dataclass, field
from fractions import Fraction
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "Nunito[wght].ttf"
FONT = HERE.parent / "src/rendering/font"

FIRST, LAST = 0x20, 0x5F
DIGITS = "0123456789"
TEXT = "".join(chr(c) for c in range(FIRST, LAST + 1))
NUMBERS = " +-./0123456789:%"

# name, weight, em size in pixels, characters, extra tracking in pixels, doc
FACES = [
    ("CAPTION", 800, 13, TEXT, 1, "Small capitals for labels, units and the status bar."),
    ("BODY", 800, 18, TEXT, 0, "Titles inside cards, toasts and sensor names."),
    ("TITLE", 900, 34, TEXT, 1, "One headline per screen, such as RIDE."),
    ("VALUE", 700, 34, NUMBERS, 0, "Secondary readings."),
    ("LARGE", 700, 50, NUMBERS, 0, "Primary readings beside each other."),
    ("HERO", 700, 78, NUMBERS, 0, "The one reading a screen is about."),
    ("SIGN_50", 900, 50, NUMBERS, 0, "The distance on a data page's turn banner."),
    ("SIGN_32", 900, 32, NUMBERS, 0, "The distance on the map's turn badge."),
    ("SIGN_27", 900, 27, "ADEINORTUV ", 0, "ARRIVED and TURN AROUND on a data page's banner."),
    ("SIGN_20", 900, 20, NUMBERS + "ADEIKMNORTUV", 0, "The chip's distance, the banner's unit and the badge's ARRIVED and TURN AROUND."),
    ("SIGN_18", 900, 18, "ABCKNRTU ", 0.5, "TURN BACK on a data page's banner."),
    ("SIGN_14", 900, 14, "ABCKMNRTU ", 0.5, "The badge's unit and TURN BACK."),
    ("SIGN_13", 900, 13, "KM ", 0, "The at-rest chip's unit."),
    ("TOTAL_60", 700, 60, NUMBERS, 0, "The ride's distance when a course is done."),
    ("TOTAL_30", 700, 30, NUMBERS, 0, "The ride's averages when a course is done."),
    ("TOTAL_11", 900, 11, "ABCDEGHIKMNPRSTVW /", 0.6, "The captions over those totals."),
    ("FOOT_20", 700, 20, NUMBERS, 0, "The distance left in the map's footer."),
]
SHIPPED_ADVANCES = {"CAPTION": {"_": 7}}


def quantize(alpha: int) -> int:
    return (alpha * 15 + 127) // 255


@dataclass
class FaintGlyphs:
    allowed: bool
    chars: list[str] = field(default_factory=list)


def rasterize(weight: int, size: int, chars: str, tracking: float, faint: FaintGlyphs):
    font = ImageFont.truetype(str(SOURCE), size)
    font.set_variation_by_axes([weight])
    cap_top = font.getbbox("H", anchor="ls")[1]
    digit_advance = max(round(font.getlength(d)) for d in DIGITS)
    glyphs = {}
    for char in chars:
        advance = round(font.getlength(char))
        left, top, right, bottom = font.getbbox(char, anchor="ls")
        width, height = right - left, bottom - top
        pixels = []
        if char != " " and width > 0 and height > 0:
            canvas = Image.new("L", (width + 4, height + 4), 0)
            ImageDraw.Draw(canvas).text((2 - left, 2 - top), char, font=font, fill=255, anchor="ls")
            box = canvas.getbbox()
            canvas = canvas.crop(box)
            width, height = canvas.size
            left += box[0] - 2
            top += box[1] - 2
            data = canvas.tobytes()
            pixels = [quantize(data[y * width + x]) for y in range(height) for x in range(width)]
            if 15 not in pixels:
                assert faint.allowed, f"{char!r} at {size}px has no solid pixel"
                faint.chars.append(char)
        else:
            width = height = 0
            left, top = 0, cap_top
        if char in DIGITS:
            left += (digit_advance - advance) // 2
            advance = digit_advance
        advance = round(advance + tracking)
        glyphs[char] = dict(x=left, y=top - cap_top, w=width, h=height, advance=advance, pixels=pixels)
    # Lift every box so no glyph starts above the face's top.
    lift = -min(g["y"] for g in glyphs.values() if g["w"])
    lift = max(lift, 0)
    for g in glyphs.values():
        g["y"] += lift
    height = max(g["y"] + g["h"] for g in glyphs.values())
    return glyphs, height, lift


def pack(pixels: list[int], width: int) -> bytes:
    rows = []
    for row in range(len(pixels) // width if width else 0):
        line = pixels[row * width : (row + 1) * width]
        if len(line) % 2:
            line = line + [0]
        rows.extend(line[i] << 4 | line[i + 1] for i in range(0, len(line), 2))
    return bytes(rows)


def round_half_up(value: Fraction) -> int:
    return int(value + Fraction(1, 2))


def scaled(scale: Fraction, size: int, tracking: float) -> tuple[int, float]:
    return round_half_up(scale * size), float(scale * Fraction(tracking).limit_denominator(10))


def output(scale: Fraction):
    if scale == 1:
        return FONT / "faces.rs"
    return FONT / f"faces_{str(float(scale)).replace('.', '_')}.rs"


def emit(scale: Fraction = Fraction(1)) -> str:
    out = [
        "//! Generated by `packages/shell/fonts/generate.py` from Nunito (SIL OFL 1.1,",
        "//! `packages/shell/fonts/OFL.txt`). Do not edit by hand.",
        "use super::{Face, Glyph};",
        "",
    ]
    total = 0
    for name, weight, size, chars, tracking, doc in FACES:
        faint = FaintGlyphs(allowed=scale != 1)
        if scale != 1:
            size, tracking = scaled(scale, size, tracking)
        glyphs, height, lift = rasterize(weight, size, chars, tracking, faint)
        if faint.chars:
            print(f"{name} at {size} px: {''.join(faint.chars)!r} have no solid pixel")
        for char, advance in SHIPPED_ADVANCES.get(name, {}).items():
            glyphs[char]["advance"] = advance if scale == 1 else round_half_up(scale * advance)
        bits = bytearray()
        entries = []
        for code in range(FIRST, LAST + 1):
            g = glyphs.get(chr(code))
            if g is None:
                entries.append("    Glyph::NONE,")
                continue
            data = pack(g["pixels"], g["w"])
            assert len(bits) + len(data) < 1 << 16, name
            assert -128 <= g["x"] < 128
            entries.append(
                f"    Glyph {{ offset: {len(bits)}, x: {g['x']}, y: {g['y']}, "
                f"w: {g['w']}, h: {g['h']}, advance: {g['advance']} }}, // {chr(code)!r}"
            )
            bits += data
        total += len(bits) + len(entries) * 8
        out += [
            f"/// {doc} Nunito {weight} at {size} px; {height} px tall.",
            f"pub static {name}: Face = Face {{",
            f"    height: {height},",
            f"    cap: {next(glyphs[c]['h'] for c in 'H0EIKNT' if c in glyphs)},",
            f"    top: {lift},",
            f"    glyphs: &{name}_GLYPHS,",
            f"    bits: &{name}_BITS,",
            "};",
            f"static {name}_GLYPHS: [Glyph; {LAST - FIRST + 1}] = [",
            *entries,
            "];",
            f"static {name}_BITS: [u8; {len(bits)}] = [",
        ]
        for i in range(0, len(bits), 20):
            out.append("    " + " ".join(f"{b:#04x}," for b in bits[i : i + 20]))
        out += ["];", ""]
    names = ", ".join(f"&{face[0]}" for face in FACES)
    out += [
        "/// Every face, for tools that read text back from frames.",
        f"pub static FACES: [&Face; {len(FACES)}] = [{names}];",
        "",
    ]
    out.insert(3, f"// {total} bytes of glyph data and tables.")
    return "\n".join(out)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--scale", type=Fraction, default=Fraction(1), help="a scale step, such as 0.75")
    scale = parser.parse_args().scale
    target = output(scale)
    target.write_text(emit(scale))
    print(f"wrote {target.relative_to(HERE.parents[1])}")
