import pytest

from cycling_devtools import stamp


def test_every_copy_of_the_placeholder_becomes_the_commit(tmp_path):
    elf = tmp_path / 'cycling-os'
    elf.write_bytes(b'\x7fELF..commit=zcyclingzcom..zcyclingzcom\x00')
    out = tmp_path / 'cycling.elf'

    assert stamp.main([str(elf), str(out), '--placeholder', 'zcyclingzcom', '--commit', '374401daa44d']) == 0

    assert out.read_bytes() == b'\x7fELF..commit=374401daa44d..374401daa44d\x00'
    assert elf.read_bytes().count(b'zcyclingzcom') == 2


def test_an_elf_without_the_placeholder_fails_the_build(tmp_path, capsys):
    elf = tmp_path / 'cycling-os'
    elf.write_bytes(b'\x7fELF..commit=unknown\x00')

    assert stamp.main([str(elf), str(tmp_path / 'out'), '--placeholder', 'zcyclingzcom', '--commit', '374401daa44d']) == 1

    assert 'no longer embeds CYCLING_BUILD_COMMIT' in capsys.readouterr().err
    assert not (tmp_path / 'out').exists()


def test_a_commit_of_another_length_would_shift_the_image():
    with pytest.raises(ValueError, match='differ in length'):
        stamp.stamp(b'zcyclingzcom', b'zcyclingzcom', b'374401daa44d0')
