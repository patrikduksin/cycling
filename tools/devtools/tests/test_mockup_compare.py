import pytest
from PIL import Image, ImageDraw

from cycling_devtools.sim import mockup

SCREENS = mockup.screens()


def rendered(screen, edit=None):
    source = (mockup.MOCKUPS / f'{screen}.svg').read_text()
    if edit:
        old, new = edit
        assert source.count(old) == 1
        source = source.replace(old, new)
    return mockup.render(source).convert('RGB')


def failures(screen, actual):
    report, _ = mockup.compare(screen, actual, mockup.rules_of(screen))
    return report.failures


def test_there_are_seven_mockups_each_with_rules():
    assert len(SCREENS) == 7
    for screen in SCREENS:
        mockup.rules_of(screen)


@pytest.mark.parametrize('screen', SCREENS)
def test_every_mockup_matches_its_own_render(screen):
    assert failures(screen, rendered(screen)) == []


@pytest.mark.parametrize(
    ('screen', 'other'),
    [
        ('ride-turns', 'ride-offcourse'),
        ('ride-straight', 'ride-turns'),
        ('ride-offcourse', 'arrived'),
        ('map-approach', 'map-close'),
        ('map-close', 'map-offcourse'),
    ],
)
def test_another_screen_fails(screen, other):
    assert failures(screen, rendered(other))


def test_a_badge_one_pixel_wider_fails_on_its_bounds():
    wider = rendered('map-approach', ('width="137" height="60"', 'width="138" height="60"'))
    assert any('rect x="8": fills (8, 30, 146, 90)' in f for f in failures('map-approach', wider))


def test_a_banner_glyph_two_pixels_low_fails():
    low = rendered('ride-turns', ('translate(52.75,63.00)', 'translate(52.75,65.00)'))
    assert any(f.startswith('g transform') for f in failures('ride-turns', low))


def test_banner_digits_two_pixels_right_fail():
    shifted = rendered('ride-turns', ('x="89.75" y="80.625"', 'x="91.75" y="80.625"'))
    assert any('text "120"' in f for f in failures('ride-turns', shifted))


def test_free_width_readings_hold_their_edge_and_rows():
    other_digits = rendered('ride-turns', ('>24.6<', '>9.8<'))
    assert failures('ride-turns', other_digits) == []
    lower = rendered('ride-turns', ('x="24" y="149"', 'x="24" y="152"'))
    assert any('text "24.6"' in f for f in failures('ride-turns', lower))


def test_masked_clock_and_dots_may_differ():
    later = rendered('ride-turns', ('>7:41<', '>12:59<'))
    moved = rendered('ride-turns', ('<circle cx="62" cy="10.5" r="3" fill="#fff"/>', ''))
    assert failures('ride-turns', later) == []
    assert failures('ride-turns', moved) == []


def test_real_roads_are_excused_but_other_colours_on_the_ground_are_not():
    actual = rendered('map-approach')
    draw = ImageDraw.Draw(actual)
    draw.line([(150, 150), (230, 230)], fill=(0x44, 0x44, 0x48), width=9)
    assert failures('map-approach', actual) == []
    draw.line([(150, 230), (230, 150)], fill=(0x80, 0x20, 0x20), width=9)
    assert any('#101012' in f for f in failures('map-approach', actual))


def test_the_blended_edge_of_a_road_past_the_screen_edge_is_excused():
    actual = rendered('map-approach')
    draw = ImageDraw.Draw(actual)
    edge = sorted(mockup.mixes((0x44, 0x44, 0x48), mockup.GROUND))[3]
    eight_bit = (edge[0] << 3, edge[1] << 2, edge[2] << 3)
    draw.line([(238, 150), (238, 160)], fill=eight_bit, width=3)
    assert failures('map-approach', actual) == []
    draw.line([(238, 240), (238, 250)], fill=(0x30, 0x10, 0x10), width=3)
    assert any('#101012' in f for f in failures('map-approach', actual))


def test_an_element_with_several_paints_must_be_named(tmp_path, monkeypatch):
    monkeypatch.setattr(mockup, 'MOCKUPS', tmp_path)
    (tmp_path / 'box.svg').write_text(
        mockup.wrap(
            '<rect width="240" height="320" fill="#000"/><rect x="10" y="10" width="50" height="40" '
            'fill="#2c2c2e" stroke="#fff" stroke-width="2"/>'
        )
    )
    actual = mockup.render((tmp_path / 'box.svg').read_text()).convert('RGB')
    assert failures_with('box', actual, {}) == ['rect x="10": several paints and no rule names it']
    assert failures_with('box', actual, {'mask': [{'match': 'stroke="#fff"'}]}) == []
    with pytest.raises(ValueError, match='no element matches'):
        failures_with('box', actual, {'mask': [{'text': 'G-21'}]})


def failures_with(screen, actual, rules):
    report, _ = mockup.compare(screen, actual, rules)
    return report.failures


def test_a_checkpoint_frame_is_cut_to_the_panel(tmp_path):
    panel = rendered('arrived')
    frame = Image.new('RGB', (900, 720), (0x14, 0x18, 0x1D))
    frame.paste(panel.resize((480, 640), Image.Resampling.NEAREST), (mockup.MARGIN, mockup.MARGIN))
    frame.save(tmp_path / 'frame.png')
    assert mockup.checkpoint_image(tmp_path / 'frame.png').tobytes() == panel.tobytes()


def test_the_command_finds_checkpoints_by_name_and_writes_sheets(tmp_path, capsys):
    sim, out = tmp_path / 'sim', tmp_path / 'out'
    (sim / 'a_turn_scenario').mkdir(parents=True)
    rendered('ride-turns').save(sim / 'a_turn_scenario' / '02-ride-turns.png')
    rendered('ride-offcourse').save(sim / 'a_turn_scenario' / '03-ride-straight.png')

    code = mockup.main(['ride-turns', 'ride-straight', 'arrived', '--sim', str(sim), '--out', str(out)])

    lines = capsys.readouterr().out.splitlines()
    assert code == 1
    assert any(line.startswith('ride-turns: pass') for line in lines)
    assert any(line.startswith('ride-straight: FAIL') for line in lines)
    assert 'arrived: FAIL 0 checkpoints named arrived' in '\n'.join(lines)
    assert lines[-1].startswith('1/3 screens match')
    assert {p.name for p in (out / 'ride-straight').iterdir()} == {'expected.png', 'actual.png', 'diff.png'}


def test_a_centred_caption_is_checked_apart_from_its_neighbours_on_the_row():
    moved = rendered('arrived', ('x="120" y="270"', 'x="126" y="270"'))
    assert any('text "AVG W"' in f for f in failures('arrived', moved))
