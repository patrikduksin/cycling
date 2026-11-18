import stat

from cycling_devtools import twin, upgrade


def checkout(tmp_path, version=7):
    root = tmp_path / 'checkout'
    source = root / 'packages/services/src'
    source.mkdir(parents=True)
    (source / 'trace.rs').write_text(f"/// The header's first byte.\npub const VERSION: u8 = {version};\n")
    return root


def trace(directory, version, build='ab' * 20):
    path = directory / f'000056-v{version}.trace'
    path.write_bytes(b'\x40\x00' + bytes([version]) + bytes.fromhex(build) + b'rest')
    return path


def dumper(tmp_path, text):
    tool = tmp_path / 'old-trace'
    (tmp_path / 'dumped').write_text(text)
    tool.write_text(f'#!/bin/sh\n[ "$1" = dump ] && cat {tmp_path / "dumped"}\n')
    tool.chmod(tool.stat().st_mode | stat.S_IEXEC)
    return tool


def test_a_trace_in_this_format_is_left_as_it_is(tmp_path):
    root = checkout(tmp_path)
    path = trace(tmp_path, 7)
    never = lambda *_: (_ for _ in ()).throw(AssertionError('no build'))  # noqa: E731
    found = upgrade.upgrade(path, root=root, tool=never)
    assert found.outcome == 'already format 7'
    assert sorted(p.name for p in tmp_path.glob('*.trace')) == [path.name]


def test_a_trace_from_before_the_world_log_is_unreadable_and_untouched(tmp_path):
    root = checkout(tmp_path)
    path = trace(tmp_path, 2)
    never = lambda *_: (_ for _ in ()).throw(AssertionError('no build'))  # noqa: E731
    found = upgrade.upgrade(path, root=root, tool=never)
    assert found.outcome == 'unreadable, left untouched: format 2 predates the world log'


def test_a_trace_whose_build_cannot_dump_is_unreadable_and_untouched(tmp_path):
    root = checkout(tmp_path)
    path = trace(tmp_path, 6)
    before = path.read_bytes()

    def incapable(build, patch, root):
        raise twin.Incapable(f'the trace tool of {build[:12]} has no dump')

    found = upgrade.upgrade(path, root=root, tool=incapable)
    assert found.outcome.startswith('unreadable, left untouched: the trace tool of abababababab has no dump')
    assert path.read_bytes() == before
    assert not path.with_name(f'{path.stem}.v7.trace').exists()


def test_the_recorded_build_dumps_and_this_build_writes_the_upgrade_beside_it(tmp_path):
    root = checkout(tmp_path)
    path = trace(tmp_path, 6)
    seen = {}

    def tool(build, patch, root):
        seen['build'], seen['patch'] = build, patch
        return dumper(tmp_path, '{"dump":1}\n')

    def reencode(dump, out, root):
        seen['dump'] = dump.read_text()
        out.write_bytes(b'v7')
        stopped = {'at_ms': 2_000, 'record': 'Ant::take_packet', 'why': 'this build does not read the bytes'}
        return {'records': 40, 'stopped': stopped}

    found = upgrade.upgrade(path, root=root, patch=b'backport', tool=tool, reencode=reencode)
    assert seen == {'build': 'ab' * 20, 'patch': b'backport', 'dump': '{"dump":1}\n'}
    assert found.upgraded == str(path.with_name(f'{path.stem}.v7.trace'))
    assert found.outcome == 'upgraded to format 7, cut short'
    assert 'ends before its pass at 2000 ms: Ant::take_packet' in upgrade.described(found)


def test_a_trace_whose_first_record_cannot_be_carried_writes_nothing(tmp_path):
    root = checkout(tmp_path)
    path = trace(tmp_path, 6)

    def reencode(dump, out, root):
        out.write_bytes(b'header only')
        return {'records': 0, 'stopped': {'at_ms': 0, 'record': 'InputSource::controls', 'why': 'D3'}}

    found = upgrade.upgrade(path, root=root, tool=lambda *_: dumper(tmp_path, ''), reencode=reencode)
    assert found.outcome.startswith('not upgraded, left untouched')
    assert not path.with_name(f'{path.stem}.v7.trace').exists()


def test_an_unarchived_build_is_reported_by_the_tool_it_would_need(tmp_path):
    root = checkout(tmp_path)
    (root / '.git').mkdir()
    path = trace(tmp_path, 6)
    found = upgrade.upgrade(path, root=root)
    assert found.outcome == 'unreadable, left untouched: build abababababab is not archived on this machine'
