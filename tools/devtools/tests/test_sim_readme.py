from cycling_devtools.sim import compare, readme
from test_sim_compare import png


def test_panel_cuts_the_screen_out_of_a_checkpoint_frame():
    width, height = readme.WIDTH + 3 * readme.MARGIN + 7, readme.HEIGHT + 2 * readme.MARGIN + 40
    frame = bytearray(width * height * 3)
    for y in range(height):
        for x in range(width):
            inside = (
                readme.MARGIN <= x < readme.MARGIN + readme.WIDTH and readme.MARGIN <= y < readme.MARGIN + readme.HEIGHT
            )
            frame[(y * width + x) * 3 : (y * width + x) * 3 + 3] = (x % 256, y % 256, 9) if inside else (1, 2, 3)

    header, rows = compare.pixels(readme.panel(png(bytes(frame), width, filter_type=2)))

    assert header[:8] == readme.WIDTH.to_bytes(4, 'big') + readme.HEIGHT.to_bytes(4, 'big')
    assert rows[:3] == bytes((readme.MARGIN, readme.MARGIN, 9))
    assert rows[-3:] == bytes(((readme.MARGIN + readme.WIDTH - 1) % 256, (readme.MARGIN + readme.HEIGHT - 1) % 256, 9))
    assert bytes((1, 2, 3)) not in {rows[i : i + 3] for i in range(0, len(rows), 3)}


def test_every_shot_has_its_own_file():
    names = [name for name, _, _ in readme.SHOTS]
    assert len(names) == len(set(names))
