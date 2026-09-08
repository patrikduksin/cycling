# /// script
# requires-python = ">=3.12"
# dependencies = ["cairosvg==2.9.1", "pillow==12.3.0"]
# ///
"""Convert the attributed vector artwork to the demo's 64×64 one-bit mask."""
import io
from pathlib import Path
import cairosvg
from PIL import Image

assets = Path(__file__).resolve().parents[1] / "packages/os/assets"
image = Image.open(io.BytesIO(cairosvg.svg2png(
    url=str(assets / "rust-logo.svg"), output_width=64, output_height=64,
))).convert("RGBA")
mask = bytes(sum((1 << (7 - bit)) if image.getpixel((column + bit, row))[3] >= 128 else 0
                 for bit in range(8)) for row in range(64) for column in range(0, 64, 8))
assert 0 < sum(byte.bit_count() for byte in mask) < 4096
(assets / "rust-logo.mask").write_bytes(mask)
