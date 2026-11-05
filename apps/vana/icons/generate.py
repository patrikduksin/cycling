# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow==11.3.0", "resvg-py==0.2.1"]
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
"""

import io
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
    coverage = render((HERE / f"{name}.svg").read_text(), size)
    alpha = Image.new("L", (size, size))
    alpha.putdata([c * 17 for c in coverage])
    left, top, right, bottom = alpha.getbbox()
    width, height = right - left, bottom - top
    data = alpha.crop((left, top, right, bottom)).tobytes()
    pixels = [value // 17 for value in data]
    assert 15 in pixels, f"{name} at {size} px has no solid pixel"
    return dict(left=left, top=top, width=width, height=height, bits=pack(pixels, width))


def zoom_sheet(names: list[str], size: int, path: Path) -> None:
    """Every icon at `size`, white on black, each pixel 8 x 8."""
    cell = size + 4
    sheet = Image.new("L", (cell * len(names), cell))
    for index, name in enumerate(names):
        tile = Image.new("L", (size, size))
        tile.putdata([c * 17 for c in render((HERE / f"{name}.svg").read_text(), size)])
        sheet.paste(tile, (index * cell + 2, 2))
    sheet.resize((sheet.width * ZOOM, sheet.height * ZOOM), Image.NEAREST).save(path)


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
    variants = "\n".join(f"    {camel(name)}," for name in SENSORS)
    head = [
        "//! Generated by `apps/vana/icons/generate.py` from the SVGs beside it.",
        "//! Do not edit by hand.",
        f"// {total} bytes of coverage and tables.",
        "use super::Icon;",
        "",
        "/// A sensor's icon, one per kind of sensor VANA draws.",
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]",
        "pub enum Sensor {",
        variants,
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
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(emit())
    print(f"wrote {OUTPUT.relative_to(HERE.parents[2])}")
