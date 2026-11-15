"""Compare simulator checkpoints with the course navigation mockups.

    cycling mockup-compare [SCREEN ...] [--checkpoint PNG|SVG] [--out DIR]

With no screen it compares all seven. A checkpoint is a simulator frame, a
240 x 320 PNG or an SVG (to compare two mockups). It writes expected.png,
actual.png and diff.png per screen and exits non-zero naming each failing
element.
"""

import argparse
import io
import re
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

import resvg_py
from PIL import Image, ImageChops, ImageFilter

from cycling_devtools.workspace import ROOT

MOCKUPS = ROOT / 'docs/design/course-navigation'
FONT = ROOT / 'packages/shell/fonts/Nunito[wght].ttf'
SIM = ROOT / '.local/sim/latest'
OUT = ROOT / '.local/mockup'
SIZE = (240, 320)
GROUND = (0x10, 0x10, 0x12)
MARGIN, SCALE = 20, 2

ELEMENT = re.compile(
    r'<g\b.*?</g>|<text\b.*?</text>|<(?:rect|circle|ellipse|polyline|polygon|path|line)\b[^>]*/>',
    re.S,
)
PAINT = re.compile(r'(?:fill|stroke)="(#[0-9a-fA-F]{3,6})"')


@dataclass
class Element:
    source: str
    mask: Image.Image
    paint: tuple[int, int, int] | None
    rule: str = 'exact'
    visible: Image.Image | None = None

    @property
    def name(self):
        if text := re.search(r'>([^<]+)</text>', self.source):
            return f'text "{text.group(1)}"'
        head = self.source.split(None, 1)[0].lstrip('<')
        where = re.search(r'\b(?:x|cx|transform)="[^"]*"', self.source)
        return f'{head} {where.group(0) if where else ""}'.strip()


@dataclass
class Report:
    failures: list[str] = field(default_factory=list)
    checked: int = 0


def render(svg, font=FONT):
    data = resvg_py.svg_to_bytes(svg_string=svg, font_files=[str(font)], width=SIZE[0], height=SIZE[1])
    return Image.open(io.BytesIO(bytes(data))).convert('RGBA')


def wrap(body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SIZE[0]} {SIZE[1]}">{body}</svg>'


def hex_colour(value):
    value = value.lstrip('#')
    if len(value) == 3:
        value = ''.join(c * 2 for c in value)
    return tuple(int(value[i : i + 2], 16) for i in (0, 2, 4))


def rgb565(image):
    r, g, b = image.convert('RGB').split()
    return Image.merge('RGB', (r.point(lambda v: v >> 3), g.point(lambda v: v >> 2), b.point(lambda v: v >> 3)))


def quantised(paint):
    r, g, b = paint
    return (r >> 3, g >> 2, b >> 3)


def equal(panel, paint):
    return matching(panel, quantised(paint))


def matching(panel, target):
    channels = [c.point(lambda v, t=t: 255 if v == t else 0) for c, t in zip(panel.split(), target, strict=True)]
    return ImageChops.darker(ImageChops.darker(channels[0], channels[1]), channels[2])


def mixes(paint, under):
    """The RGB565 colours the firmware's 4-bit blend makes of `paint` over
    `under`, from a sliver to solid."""
    fg, bg = quantised(paint), quantised(under)
    return {tuple((f * a + b * (15 - a) + 7) // 15 for f, b in zip(fg, bg, strict=True)) for a in range(1, 16)}


def solid(mask):
    return mask.point(lambda v: 255 if v == 255 else 0)


def covered(mask):
    return mask.point(lambda v: 255 if v else 0)


def grow(mask, pixels=1):
    return mask.filter(ImageFilter.MaxFilter(2 * pixels + 1))


def shrink(mask):
    return mask.filter(ImageFilter.MinFilter(3))


def points(mask):
    width = mask.width
    return [(i % width, i // width) for i, v in enumerate(mask.tobytes()) if v]


def region(rect):
    mask = Image.new('L', SIZE, 0)
    mask.paste(255, tuple(rect))
    return mask


def selects(rule, element):
    if 'text' in rule:
        return element.source.startswith('<text') and f'>{rule["text"]}</text>' in element.source
    if 'match' in rule:
        return rule['match'] in element.source
    return False


def load(mockup, rules):
    source = mockup.read_text()
    inner = source[source.index('>') + 1 : source.rindex('</svg>')]
    elements = []
    for found in ELEMENT.findall(inner):
        paints = {p.lower() for p in PAINT.findall(found)}
        paint = hex_colour(paints.pop()) if len(paints) == 1 else None
        elements.append(Element(found, render(wrap(found)).getchannel('A'), paint))
    interior_and_envelope_exclusions = Image.new('L', SIZE, 0)
    for kind in ('text', 'mask'):
        for rule in rules.get(kind, []):
            if 'region' in rule:
                interior_and_envelope_exclusions = ImageChops.lighter(
                    interior_and_envelope_exclusions, region(rule['region'])
                )
                continue
            chosen = [e for e in elements if selects(rule, e)]
            if not chosen:
                raise ValueError(f'{mockup.name}: no element matches rule {rule}')
            for element in chosen:
                element.rule = kind
    over = Image.new('L', SIZE, 0)
    for element in reversed(elements):
        element.visible = ImageChops.subtract(element.mask, over)
        if element.rule == 'mask':
            interior_and_envelope_exclusions = ImageChops.lighter(
                interior_and_envelope_exclusions, covered(element.visible)
            )
        elif element.rule == 'text':
            interior_and_envelope_exclusions = ImageChops.lighter(
                interior_and_envelope_exclusions, expanded_text_region(element)
            )
        over = ImageChops.lighter(over, covered(element.mask))
    return elements, interior_and_envelope_exclusions


def checkpoint_image(path):
    if path.suffix == '.svg':
        return render(path.read_text()).convert('RGB')
    image = Image.open(path).convert('RGB')
    if image.size == SIZE:
        return image
    panel = image.crop((MARGIN, MARGIN, MARGIN + SIZE[0] * SCALE, MARGIN + SIZE[1] * SCALE))
    return panel.resize(SIZE, Image.Resampling.NEAREST)


def compare(screen, actual, rules):
    elements, interior_and_envelope_exclusions = load(MOCKUPS / f'{screen}.svg', rules)
    panel = rgb565(actual)
    for colour in rules.get('ignore', []):
        for mix in mixes(hex_colour(colour), GROUND):
            interior_and_envelope_exclusions = ImageChops.lighter(
                interior_and_envelope_exclusions, grow(matching(panel, mix))
            )
    interior_and_envelope_allowed = ImageChops.invert(interior_and_envelope_exclusions)
    report, diff = Report(), Image.new('L', SIZE, 0)

    def fail(message, pixels):
        report.failures.append(message)
        nonlocal diff
        diff = ImageChops.lighter(diff, pixels)

    seen = set()
    for element in elements:
        if element.rule == 'mask':
            continue
        if element.paint is None:
            fail(f'{element.name}: several paints and no rule names it', covered(element.visible))
            continue
        report.checked += 1
        here = equal(panel, element.paint)
        if element.rule == 'text':
            check_text(element, here, fail)
        else:
            interior = ImageChops.darker(shrink(solid(element.visible)), interior_and_envelope_allowed)
            wrong = ImageChops.subtract(interior, here)
            if bad := points(wrong):
                share = len(bad) / max(len(points(interior)), 1)
                fail(f'{element.name}: {len(bad)} interior px ({share:.0%}) are not {hex_of(element.paint)}', wrong)
            backdrop = element.mask.getbbox() == (0, 0, *SIZE)
            if element.source.startswith('<rect') and element.paint not in seen and not backdrop:
                others = [e for e in elements if e is not element and e.paint == element.paint]
                check_bounds(element, here, others, fail)
        seen.add(element.paint)
    for paint in seen:
        drawn = Image.new('L', SIZE, 0)
        for element in elements:
            if element.paint == paint and element.rule != 'mask':
                own = covered(element.visible) if element.rule == 'exact' else expanded_text_region(element)
                drawn = ImageChops.lighter(drawn, own)
        outside = ImageChops.darker(
            ImageChops.subtract(shrink(equal(panel, paint)), drawn), interior_and_envelope_allowed
        )
        if bad := points(outside):
            x, y = bad[0]
            fail(f'{hex_of(paint)}: {len(bad)} px drawn where the mockup has none, first at ({x}, {y})', outside)
    return report, diff


def check_text(element, here, fail):
    want = ink_box(element)
    band = expanded_text_region(element)
    found = ImageChops.darker(here, band).getbbox()
    if found is None:
        fail(f'{element.name}: no ink in {hex_of(element.paint)} near ({want[0]}, {want[1]})', band)
        return
    if (
        abs(anchored(element, found) - anchored(element, want)) > 1
        or abs(found[1] - want[1]) > 1
        or abs(found[3] - want[3]) > 1
    ):
        fail(f'{element.name}: ink {found}, mockup {want}', region(found))


def ink_box(element):
    return solid(element.mask).getbbox() or covered(element.mask).getbbox()


def anchor(element):
    found = re.search(r'text-anchor="(end|middle)"', element.source)
    return found.group(1) if found else 'start'


def anchored(element, rect):
    return {'start': rect[0], 'middle': (rect[0] + rect[2]) / 2, 'end': rect[2]}[anchor(element)]


def expanded_text_region(element):
    left, top, right, bottom = ink_box(element)
    expanded_width = (right - left) * 3 // 2 + 2
    columns = {
        'start': (left - 2, left + expanded_width),
        'middle': ((left + right) // 2 - expanded_width // 2, (left + right) // 2 + expanded_width // 2),
        'end': (right - expanded_width, right + 2),
    }[anchor(element)]
    return region((max(columns[0], 0), max(top - 2, 0), min(columns[1], SIZE[0]), min(bottom + 2, SIZE[1])))


def check_bounds(element, here, others, fail):
    for other in others:
        here = ImageChops.subtract(here, grow(covered(other.mask)))
    want = solid(element.visible).getbbox()
    if want is None:
        return
    left, top, right, bottom = want
    near = region((max(left - 16, 0), max(top - 16, 0), min(right + 16, SIZE[0]), min(bottom + 16, SIZE[1])))
    got = ImageChops.darker(here, near).getbbox()
    if got != want:
        fail(f'{element.name}: fills {got}, mockup {want}', ImageChops.difference(region(want), region(got or want)))


def hex_of(paint):
    return '#' + ''.join(f'{c:02x}' for c in paint)


def rules_of(screen):
    return tomllib.loads((MOCKUPS / f'{screen}.rules.toml').read_text())


def checkpoint_of(screen, sim):
    found = sorted(sim.glob(f'*/[0-9][0-9]-{screen}.png'))
    if len(found) != 1:
        raise LookupError(f'{len(found)} checkpoints named {screen} under {sim}')
    return found[0]


def write(out, screen, actual, diff):
    folder = out / screen
    folder.mkdir(parents=True, exist_ok=True)
    render((MOCKUPS / f'{screen}.svg').read_text()).convert('RGB').save(folder / 'expected.png')
    actual.save(folder / 'actual.png')
    marked = actual.point(lambda v: v // 3)
    marked.paste((255, 40, 40), mask=diff)
    marked.save(folder / 'diff.png')


def screens():
    return sorted(p.stem for p in MOCKUPS.glob('*.svg'))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('screens', nargs='*', help='mockup names, such as ride-turns; default every one')
    parser.add_argument('--checkpoint', type=Path, help='compare one screen with this PNG or SVG')
    parser.add_argument('--sim', type=Path, default=SIM, help='simulator output holding the checkpoints')
    parser.add_argument('--out', type=Path, default=OUT)
    args = parser.parse_args(argv)
    chosen = args.screens or screens()
    if args.checkpoint and len(chosen) != 1:
        parser.error('--checkpoint compares one screen')
    failed = 0
    for screen in chosen:
        rules = rules_of(screen)
        try:
            path = args.checkpoint or checkpoint_of(screen, args.sim)
        except LookupError as error:
            print(f'{screen}: FAIL {error}')
            failed += 1
            continue
        actual = checkpoint_image(path)
        report, diff = compare(screen, actual, rules)
        write(args.out, screen, actual, diff)
        for message in report.failures:
            print(f'{screen}: FAIL {message}')
        verdict = 'pass' if not report.failures else 'FAIL'
        print(f'{screen}: {verdict}, {report.checked} elements checked against {path}')
        failed += bool(report.failures)
    print(f'{len(chosen) - failed}/{len(chosen)} screens match; sheets in {args.out}')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
