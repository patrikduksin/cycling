from cycling_devtools.sim import screens
from test_sim_compare import IMAGE, png


def write(root, cells):
    for name, data in cells.items():
        path = root / f'{name}.png'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


def test_cells_pair_by_name_and_changed_screens_lead_the_index(tmp_path):
    same, other = png(IMAGE, 4), png(bytes(reversed(IMAGE)), 4)
    write(
        tmp_path / 'before',
        {'home/top@240x320': same, 'paused/free-ride@180x240': same, 'gone/x@240x320': same},
    )
    write(
        tmp_path / 'after',
        {'home/top@240x320': png(IMAGE, 4, level=1), 'paused/free-ride@180x240': other, 'fresh/y@240x320': same},
    )
    found = screens.compared(tmp_path / 'before', tmp_path / 'after')
    states = {s: {v: {p: c.state for p, c in row.items()} for v, row in rows.items()} for s, rows in found.items()}
    assert states == {
        'fresh': {'y': {'240x320': 'new'}},
        'gone': {'x': {'240x320': 'removed'}},
        'home': {'top': {'240x320': 'unchanged'}},
        'paused': {'free-ride': {'180x240': 'changed'}},
    }
    sheets = {s: tmp_path / f'{s}.png' for s in found}
    markdown, changed, total = screens.index(
        found, sheets, ('main', 'abc', 'subject', 'def'), {'this checkout': ['menus']}, [['a/b', 'why']]
    )
    assert (changed, total) == (1, 4)
    assert markdown.index('## fresh') < markdown.index('## Unchanged') < markdown.index('[home]')
    assert '`paused/free-ride@180x240` changed' in markdown
    assert 'failed on this checkout' in markdown and 'menus' in markdown
    assert '`a/b`: why' in markdown
