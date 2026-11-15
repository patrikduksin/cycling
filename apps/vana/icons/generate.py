# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow==13.0.0", "resvg-py==0.5.0"]
# ///
"""Rasterize VANA's icons into 4-bit coverage for `screens::icons`.

Run `uv run apps/vana/icons/generate.py` after changing an SVG beside this
file or a size below; it rewrites `apps/vana/src/screens/icons/art.rs`. Each
SVG draws on a 24-unit square in `currentColor`. An icon is rasterized at
each pixel size it is drawn at, so strokes stay sharp rather than scaled,
and cropped to its ink: screens paint it in any colour, blended over what
lies behind, as text is.

The whole SVG is rendered at once, so parts that overlap make one shape with
no seam. Parts that do not touch in the drawing must keep a clear pixel
between them at every size, or the script stops. `--zoom DIR` also writes an
8x nearest-neighbour sheet of each size to DIR for review.

The course sign's glyphs come from the mockups in docs/design/course-navigation
instead: each is drawn at the scale and sub-pixel centre the mockup gives it in
each form of the sign, so a painted glyph is the mockup's own pixels. A left
glyph is drawn mirrored, as the mockup does, rather than flipped at paint: a
centre at a quarter pixel cannot be flipped onto pixel columns.
"""

import io
import math
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

import resvg_py
from PIL import Image

HERE = Path(__file__).resolve().parent
OUTPUT = HERE.parent / "src/screens/icons/art.rs"

SENSORS = [
    "heart",
    "power",
    "speed",
    "cadence",
    "speed-cadence",
    "radar",
    "shifting",
    "trainer",
    "lights",
    "generic",
]
SENSOR_SIZES = [22, 26, 34]
MARKS = [("refresh", 18), ("plus", 18), ("arrow", 28)]

# Sign glyph art in the mockups' units, centred on the origin.
TURN = (
    '<path d="M-15 19 V-1 Q-15 -11 -5 -11 H4" fill="none" stroke="#fff" stroke-width="10" '
    'stroke-linecap="round" stroke-linejoin="round"/>'
    '<polygon points="3,-23 22,-11 3,1" fill="#fff" stroke="#fff" stroke-width="2" stroke-linejoin="round"/>'
)
# No mockup draws a sharp turn: this is TURN's stem and head with the bend
# carried on to 135 degrees, for review against the mockups.
SHARP = (
    '<path d="M-15 20 V-4 Q-15 -18 -6 -9 L2 -1" fill="none" stroke="#fff" stroke-width="10" '
    'stroke-linecap="round" stroke-linejoin="round"/>'
    '<polygon points="10.5,-9.5 -6.5,7.5 15.4,12.4" fill="#fff" stroke="#fff" stroke-width="2" '
    'stroke-linejoin="round"/>'
)
UTURN = (
    '<path d="M14 19 V-7 A12 12 0 0 0 -10 -7 V4" fill="none" stroke="#fff" stroke-width="10" '
    'stroke-linecap="round"/>'
    '<polygon points="-23,3 3,3 -10,23" fill="#fff" stroke="#fff" stroke-width="2" stroke-linejoin="round"/>'
)
FLAG = (
    '<path d="M-17 20.5 V-20.5" stroke="#fff" stroke-width="7" stroke-linecap="round"/>'
    '<path d="M-13 -22 H20 L12 -10.5 L20 1 H-13 Z" fill="#fff" stroke="#fff" stroke-width="2" '
    'stroke-linejoin="round"/>'
)
# The sign's forms: the glyph's scale and the top-left corner of the fill it
# sits on, from ride-turns, map-approach and ride-straight.
FORMS = [("Banner", 1.25, (0, 22)), ("Badge", 0.9167, (8, 30)), ("Chip", 0.375, (24, 72))]
# Glyph centres per form, in screen pixels. A turn's centre is the mockups';
# the off-course U-turn's is ride-offcourse's and map-offcourse's; the flag's
# banner centre is arrived's. No mockup draws these in the other forms, so
# those keep the banner's offset from the turn's centre, scaled.
TURN_AT = [(52.75, 63.0), (34.33, 60.0), (37.44, 86.0)]
BACK_AT = [(54.0, 63.0), (38.0, 60.0), None]
FLAG_AT = [(49.62, 63.0), None, None]
# name, art, 1 or -1 for mirrored, centres per form
SIGN_GLYPHS = [
    ("Right", TURN, 1, TURN_AT),
    ("Left", TURN, -1, TURN_AT),
    ("SharpRight", SHARP, 1, TURN_AT),
    ("SharpLeft", SHARP, -1, TURN_AT),
    ("UTurnLeft", UTURN, 1, TURN_AT),
    ("UTurnRight", UTURN, -1, TURN_AT),
    ("Back", UTURN, 1, BACK_AT),
    ("Flag", FLAG, 1, FLAG_AT),
]
# Art units from a glyph's centre to the edge of its ink, with room to spare.
REACH = 26
# The map's rider arrow, centred on the puck's discs; map-approach paints it
# ACCENT and map-offcourse AMBER.
PUCK = '<polygon points="0,-8.5 7,7 0,3 -7,7" fill="#fff" stroke="#fff" stroke-width="1.5" stroke-linejoin="round"/>'
PUCK_SIZE = 20


def quantize(alpha: int) -> int:
    return (alpha * 15 + 127) // 255


SVG = "http://www.w3.org/2000/svg"
ZOOM = 8
MIN_INK = 3


def render(svg: str, size: int) -> list[int]:
    """Coverage 0..=15 of `svg` drawn in a `size` square, row by row."""
    svg = svg.replace("<svg ", f'<svg width="{size}" height="{size}" ', 1)
    png = Image.open(io.BytesIO(bytes(resvg_py.svg_to_bytes(svg_string=svg))))
    assert png.size == (size, size), (size, png.size)
    return [quantize(a) for a in png.getchannel("A").tobytes()]


def parts(name: str) -> list[str]:
    """Each drawn element of an icon as an SVG of its own, with the attributes
    of its group; a stroked path is split into its subpaths."""
    ET.register_namespace("", SVG)
    root = ET.fromstring((HERE / f"{name}.svg").read_text())
    leaves = []
    for child in root:
        group = child.attrib if child.tag == f"{{{SVG}}}g" else {}
        for leaf in child if group else [child]:
            attrib = group | leaf.attrib
            d = attrib.get("d", "")
            if attrib.get("fill") == "none" and d.count("M") > 1:
                for sub in d.split("M")[1:]:
                    leaves.append((leaf.tag, attrib | {"d": "M" + sub}))
            else:
                leaves.append((leaf.tag, attrib))
    svgs = []
    for tag, attrib in leaves:
        shell = ET.Element(root.tag, root.attrib)
        ET.SubElement(shell, tag, attrib)
        svgs.append(ET.tostring(shell, encoding="unicode"))
    return svgs


def check_gaps(name: str, sizes: list[int]) -> None:
    """Parts that do not touch in the drawing keep a clear pixel between them."""
    pieces = parts(name)
    fine = [render(piece, 240) for piece in pieces]
    group = list(range(len(pieces)))

    def find(i: int) -> int:
        while group[i] != i:
            i = group[i]
        return i

    for i in range(len(pieces)):
        for j in range(i + 1, len(pieces)):
            if any(a > 7 and b > 7 for a, b in zip(fine[i], fine[j])):
                group[find(j)] = find(i)
    for size in sizes:
        masks = [render(piece, size) for piece in pieces]
        for i in range(len(pieces)):
            for j in range(i + 1, len(pieces)):
                if find(i) == find(j):
                    continue
                for y in range(size):
                    for x in range(size):
                        if masks[i][y * size + x] < MIN_INK:
                            continue
                        near = [
                            masks[j][ny * size + nx]
                            for ny in range(max(y - 1, 0), min(y + 2, size))
                            for nx in range(max(x - 1, 0), min(x + 2, size))
                        ]
                        assert max(near) < MIN_INK, f"{name} at {size} px: parts {i} and {j} touch at {x},{y}"


def rasterize(name: str, size: int) -> dict:
    return crop(render((HERE / f"{name}.svg").read_text(), size), size)


def crop(coverage: list[int], size: int) -> dict:
    alpha = Image.new("L", (size, size))
    alpha.putdata([c * 17 for c in coverage])
    left, top, right, bottom = alpha.getbbox()
    width, height = right - left, bottom - top
    data = alpha.crop((left, top, right, bottom)).tobytes()
    pixels = [value // 17 for value in data]
    assert 15 in pixels, f"an icon at {size} px has no solid pixel"
    return dict(left=left, top=top, width=width, height=height, bits=pack(pixels, width))


def centres(at: list) -> list[tuple[float, float]]:
    """A glyph's centre in each form, filling a form no mockup draws it in
    from the banner's offset to the turn's centre."""
    (bx, by), (tx, ty) = at[0], TURN_AT[0]
    banner = FORMS[0][1]
    out = []
    for (_, scale, _), centre, (ux, uy) in zip(FORMS, at, TURN_AT, strict=True):
        out.append(centre or (ux + (bx - tx) / banner * scale, uy + (by - ty) / banner * scale))
    return out


def draw(art: str, size: int, x: float, y: float, mirror: int, scale: float) -> str:
    return (
        f'<svg xmlns="{SVG}" viewBox="0 0 {size} {size}">'
        f'<g transform="translate({x:.4f},{y:.4f}) scale({mirror * scale},{scale})">{art}</g></svg>'
    )


def sign_glyph(art: str, mirror: int, scale: float, fill: tuple[int, int], centre: tuple[float, float]) -> dict:
    """The glyph in a square whose top-left corner sits a whole number of
    pixels from the fill's, so the glyph keeps its centre's fraction."""
    reach = math.ceil(REACH * scale) + 1
    x, y = centre[0] - fill[0], centre[1] - fill[1]
    left, top = max(int(x) - reach, 0), max(int(y) - reach, 0)
    size = max(int(x) + reach - left, int(y) + reach - top)
    svg = draw(art, size, x - left, y - top, mirror, scale)
    coverage = render(svg, size)
    edge = [coverage[i] for i in range(size)] + [coverage[i * size] for i in range(size)]
    edge += [coverage[-1 - i] for i in range(size)] + [coverage[i * size + size - 1] for i in range(size)]
    assert max(edge) == 0, f"a sign glyph at {scale} reaches past its square"
    return dict(left=left, top=top, size=size, raster=crop(coverage, size))


def zoom_sheet(names: list[str], size: int, path: Path) -> None:
    """Every icon at `size`, white on black, each pixel 8 x 8."""
    cell = size + 4
    sheet = Image.new("L", (cell * len(names), cell))
    for index, name in enumerate(names):
        tile = Image.new("L", (size, size))
        tile.putdata([c * 17 for c in render((HERE / f"{name}.svg").read_text(), size)])
        sheet.paste(tile, (index * cell + 2, 2))
    sheet.resize((sheet.width * ZOOM, sheet.height * ZOOM), Image.NEAREST).save(path)


def sign_sheet(form: str, scale: float, fill: tuple[int, int], path: Path) -> None:
    """Every sign glyph in one form, white on its fill, each pixel 8 x 8."""
    form_index = [f for f, _, _ in FORMS].index(form)
    glyphs = [
        sign_glyph(art, mirror, scale, fill, centres(at)[form_index]) for _, art, mirror, at in SIGN_GLYPHS
    ]
    cell = max(g["size"] for g in glyphs) + 4
    sheet = Image.new("RGB", (cell * len(glyphs), cell), (10, 111, 224))
    for index, glyph in enumerate(glyphs):
        raster, size = glyph["raster"], glyph["size"]
        tile = Image.new("L", (size, size))
        alpha = Image.new("L", (raster["width"], raster["height"]))
        alpha.putdata([c * 17 for c in unpack(raster["bits"], raster["width"], raster["height"])])
        tile.paste(alpha, (raster["left"], raster["top"]))
        sheet.paste((255, 255, 255), (index * cell + 2, 2), mask=tile)
    sheet.resize((sheet.width * ZOOM, sheet.height * ZOOM), Image.NEAREST).save(path)


def unpack(bits: bytes, width: int, height: int) -> list[int]:
    stride = (width + 1) // 2
    return [bits[y * stride + x // 2] >> (0 if x % 2 else 4) & 15 for y in range(height) for x in range(width)]


def pack(pixels: list[int], width: int) -> bytes:
    rows = []
    for row in range(len(pixels) // width):
        line = pixels[row * width : (row + 1) * width]
        if len(line) % 2:
            line = line + [0]
        rows.extend(line[i] << 4 | line[i + 1] for i in range(0, len(line), 2))
    return bytes(rows)


def camel(name: str) -> str:
    return "".join(part.title() for part in name.split("-"))


def fields(size: int, raster: dict, indent: int) -> list[str]:
    """An `Icon`'s fields, as rustfmt lays them out at `indent`."""
    pad = " " * indent
    lines = [
        f"{pad}size: {size},",
        f"{pad}left: {raster['left']},",
        f"{pad}top: {raster['top']},",
        f"{pad}width: {raster['width']},",
        f"{pad}height: {raster['height']},",
        f"{pad}bits: &[",
    ]
    bits = raster["bits"]
    # rustfmt fills an array of short literals up to 100 columns.
    inner = indent + 4
    per_line = (100 - inner + 1) // 6
    for i in range(0, len(bits), per_line):
        lines.append(" " * inner + " ".join(f"{b:#04x}," for b in bits[i : i + per_line]))
    return lines + [f"{pad}],"]


def emit() -> str:
    total = 0
    body = []
    for size in SENSOR_SIZES:
        body.append(f"/// Sensor icons at {size} px, in `Sensor` order.")
        body.append(f"pub static SENSORS_{size}: [Icon; {len(SENSORS)}] = [")
        for name in SENSORS:
            raster = rasterize(name, size)
            total += len(raster["bits"]) + 8
            body += [f"    // {name}", "    Icon {", *fields(size, raster, 8), "    },"]
        body += ["];", ""]
    for name, size in MARKS:
        raster = rasterize(name, size)
        total += len(raster["bits"]) + 8
        body += [
            f"/// The {name} mark at {size} px.",
            f"pub static {name.upper()}: Icon = Icon {{",
            *fields(size, raster, 4),
            "};",
            "",
        ]
    raster = crop(render(draw(PUCK, PUCK_SIZE, PUCK_SIZE / 2, PUCK_SIZE / 2, 1, 1), PUCK_SIZE), PUCK_SIZE)
    total += len(raster["bits"]) + 8
    body += [
        "/// The map's rider arrow; its square's centre is the puck's.",
        "pub static PUCK: Icon = Icon {",
        *fields(PUCK_SIZE, raster, 4),
        "};",
        "",
        "/// Each `SignGlyph`, in `SignForm` order.",
        f"pub(super) static SIGN_GLYPHS: [[SignArt; {len(FORMS)}]; {len(SIGN_GLYPHS)}] = [",
    ]
    for name, art, mirror, at in SIGN_GLYPHS:
        body += [f"    // {name}", "    ["]
        for (form, scale, fill), centre in zip(FORMS, centres(at), strict=True):
            glyph = sign_glyph(art, mirror, scale, fill, centre)
            total += len(glyph["raster"]["bits"]) + 10
            body += [
                f"        // {form}, centred at ({centre[0]:.2f}, {centre[1]:.2f})",
                "        SignArt {",
                f"            left: {glyph['left']},",
                f"            top: {glyph['top']},",
                "            icon: Icon {",
                *fields(glyph["size"], glyph["raster"], 16),
                "            },",
                "        },",
            ]
        body.append("    ],")
    body += ["];", ""]
    variants = "\n".join(f"    {camel(name)}," for name in SENSORS)
    glyphs = "\n".join(f"    {name}," for name, *_ in SIGN_GLYPHS)
    head = [
        "//! Generated by `apps/vana/icons/generate.py` from the SVGs beside it.",
        "//! Do not edit by hand.",
        f"// {total} bytes of coverage and tables.",
        "use super::{Icon, SignArt};",
        "",
        "/// A sensor's icon, one per kind of sensor VANA draws.",
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]",
        "pub enum Sensor {",
        variants,
        "}",
        "",
        "/// A course sign's glyph. A left glyph is drawn mirrored, not flipped.",
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]",
        "pub enum SignGlyph {",
        glyphs,
        "}",
        "",
    ]
    return "\n".join(head + body)


if __name__ == "__main__":
    for name in SENSORS:
        check_gaps(name, SENSOR_SIZES)
    for name, size in MARKS:
        check_gaps(name, [size])
    if "--zoom" in sys.argv:
        zoom = Path(sys.argv[sys.argv.index("--zoom") + 1])
        zoom.mkdir(parents=True, exist_ok=True)
        for size in SENSOR_SIZES:
            zoom_sheet(SENSORS, size, zoom / f"sensors-{size}.png")
        for size in sorted({size for _, size in MARKS}):
            marks = [name for name, mark in MARKS if mark == size]
            zoom_sheet(marks, size, zoom / f"marks-{size}.png")
        for form, scale, fill in FORMS:
            sign_sheet(form, scale, fill, zoom / f"sign-{form.lower()}.png")
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(emit())
    print(f"wrote {OUTPUT.relative_to(HERE.parents[2])}")
