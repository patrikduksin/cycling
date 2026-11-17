import pytest

from cycling_devtools import stamp


def test_every_copy_of_each_placeholder_becomes_its_value(tmp_path):
    elf = tmp_path / 'cycling-os'
    elf.write_bytes(b'\x7fELF..commit=zcyclingzcom..zcyclingzcom..build=zhashz\x00')
    out = tmp_path / 'cycling.elf'

    assert stamp.main([str(elf), str(out), '--stamp', 'zcyclingzcom=374401daa44d', '--stamp', 'zhashz=0a1b2c']) == 0

    assert out.read_bytes() == b'\x7fELF..commit=374401daa44d..374401daa44d..build=0a1b2c\x00'
    assert elf.read_bytes().count(b'zcyclingzcom') == 2


def test_an_elf_without_a_placeholder_fails_the_build(tmp_path, capsys):
    elf = tmp_path / 'cycling-os'
    elf.write_bytes(b'\x7fELF..commit=zcyclingzcom..build=unknown\x00')

    assert (
        stamp.main(
            [str(elf), str(tmp_path / 'out'), '--stamp', 'zcyclingzcom=374401daa44d', '--stamp', 'zhashz=0a1b2c']
        )
        == 1
    )

    assert "'zhashz' is not in the ELF" in capsys.readouterr().err
    assert not (tmp_path / 'out').exists()


def test_a_value_of_another_length_would_shift_the_image():
    with pytest.raises(ValueError, match='differ in length'):
        stamp.stamp(b'zcyclingzcom', b'zcyclingzcom', b'374401daa44d0')


def test_a_stamp_without_a_value_is_refused_before_the_elf_is_read(tmp_path, capsys):
    with pytest.raises(SystemExit) as refused:
        stamp.main([str(tmp_path / 'missing'), str(tmp_path / 'out'), '--stamp', 'zhashz'])
    assert refused.value.code == 2
    assert "'zhashz' is not PLACEHOLDER=VALUE" in capsys.readouterr().err
