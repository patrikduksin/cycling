import pytest

from cycling_devtools import stack_frames

# Captured from `xtensa-esp32s3-elf-objdump -d -C --no-show-raw-insn` of a
# release cycling-os ELF, trimmed to each function's first instructions.
FIRMWARE = """
/home/patrik/cycling/.local/elf/e28aae10feb5-sdk1-h1.elf:     file format elf32-xtensa-le


Disassembly of section .rwtext:

40378418 <sym_main>:
40378418:	.byte	0xc4
40378419:	addi.n	a0, a8, 15
4037841b:	.byte	0x42

Disassembly of section .flash.text:

4211de14 <<vana::screens::workout::View>::new>:
4211de14:	entry	a1, 0x2af0
4211de17:	mov.n	a8, a7
4211de19:	mov.n	a7, a1

421217b8 <<vana::screens::workout::View>::prepare>:
421217b8:	entry	a1, 0x110
421217bb:	mov.n	a7, a1
421217bd:	l32r	a8, 420ef04c <<qrcodegen_no_heap::QrCode>::encode_segments_to_codewords+0x2784> (2970 <VECTORS_SIZE+0x2570>)
421217c0:	add.n	a8, a2, a8
421217c2:	l8ui	a8, a8, 0

42121be8 <vana::screens::workout::fill::<<vana::screens::workout::View>::prepare::{closure#0}>>:
42121be8:	entry	a1, 0xe00
42121beb:	mov.n	a7, a1
42121bed:	movi.n	a8, 0

42121c1c <vana::screens::workout::fill::<<vana::screens::workout::View>::prepare::{closure#1}>>:
42121c1c:	entry	a1, 0x1c20
42121c1f:	mov.n	a7, a1
42121c21:	mov.n	a3, a2

420c3600 <<c606_firmware::drivers::sdmmc::Host>::reset_card>:
420c3600:	entry	a1, 64
420c3603:	mov.n	a7, a1
420c3605:	addi	a8, a1, -4
420c3608:	movsp	a1, a8
420c360b:	movi.n	a11, 0

4037d000 <lmacEndFrameExchangeSequence>:
4037d025:	bnone	a3, a14, 4037d065 <lmacEndFrameExchangeSequence+0x15>
4037d028:	entry	a7, 0x1f0
4037d02b:	movi.n	a12, 50
"""

# A 40,000 B array: past the 32,760 B `entry` immediate, LLVM allocates the
# rest from a literal (captured from a linked probe object).
LARGE = """
00400088 <big>:
  400088:	entry	a1, 32
  40008b:	l32r	a8, 400080 <big-0x8> (9c50 <big-0x3f6438>)
  40008e:	sub	a8, a1, a8
  400091:	movsp	a1, a8
  400094:	addi	a7, a1, 4
"""


def test_reads_the_entry_frame_of_each_windowed_function():
    assert stack_frames.parse(FIRMWARE) == {
        '<vana::screens::workout::View>::new': 0x2AF0,
        '<vana::screens::workout::View>::prepare': 0x110,
        'vana::screens::workout::fill::<<vana::screens::workout::View>::prepare::{closure#0}>': 0xE00,
        'vana::screens::workout::fill::<<vana::screens::workout::View>::prepare::{closure#1}>': 0x1C20,
        '<c606_firmware::drivers::sdmmc::Host>::reset_card': 64,
    }


def test_a_frame_past_the_entry_immediate_adds_its_literal():
    assert stack_frames.parse(LARGE) == {'big': 32 + 0x9C50}


def test_an_unrecognised_large_frame_prologue_fails_instead_of_undercounting():
    garbled = LARGE.replace('l32r\ta8, 400080 <big-0x8> (9c50 <big-0x3f6438>)', 'movi\ta8, 2000')
    with pytest.raises(ValueError, match='big'):
        stack_frames.parse(garbled)


def test_a_stack_move_after_a_32_byte_entry_fails_unless_it_is_the_large_frame_form():
    moved = LARGE.replace('sub\ta8, a1, a8', 'addi\ta8, a1, -4')
    with pytest.raises(ValueError, match='big'):
        stack_frames.parse(moved)


def test_the_margin_is_a_kibibyte_or_a_tenth_of_the_frame():
    assert stack_frames.margin(272) == 1024
    assert stack_frames.margin(19_456) == 1946


def test_a_repeated_name_keeps_its_largest_frame():
    twice = FIRMWARE + FIRMWARE.replace('entry\ta1, 0x110', 'entry\ta1, 0x900')
    assert stack_frames.parse(twice)['<vana::screens::workout::View>::prepare'] == 0x900


def test_the_first_matching_rule_applies_and_only_star_is_a_wildcard():
    rules = [
        stack_frames.Rule('vana::screens::workout::fill::<*>', 8000),
        stack_frames.Rule('<[u8]>::copy_from_slice', 100),
        stack_frames.Rule('*', 1000),
    ]
    frames = {frame.name: frame for frame in stack_frames.assess(stack_frames.parse(FIRMWARE), rules)}
    fill = 'vana::screens::workout::fill::<<vana::screens::workout::View>::prepare::{closure#1}>'
    assert frames[fill].rule == rules[0]
    assert not frames[fill].over
    assert frames['<vana::screens::workout::View>::new'].rule == rules[2]
    assert frames['<vana::screens::workout::View>::new'].over
    assert not rules[1].matches('<u>::copy_from_slice')


@pytest.fixture
def objdump(tmp_path):
    listing = tmp_path / 'listing.txt'
    listing.write_text(FIRMWARE)
    script = tmp_path / 'objdump'
    script.write_text(f'#!/bin/sh\ncat {listing}\n')
    script.chmod(0o755)
    return script


def run(tmp_path, objdump, budgets, *extra):
    path = tmp_path / 'stack-frames.toml'
    path.write_text(budgets)
    elf = tmp_path / 'cycling-os'
    elf.write_bytes(b'')
    return stack_frames.main(
        [str(elf), '--variant', 'sdk1-h1', '--budgets', str(path), '--objdump', str(objdump), *extra]
    )


BUDGETS = """
[sdk1-h1]
"<vana::screens::workout::View>::prepare" = 1296
"vana::screens::workout::fill::<*>" = 8192
"*" = 12000
"""


def test_frames_within_budget_pass_and_write_the_top_table(tmp_path, objdump, capsys):
    report = tmp_path / 'stack-frames.md'
    assert run(tmp_path, objdump, BUDGETS, '--report', str(report), '--top', '2') == 0
    table = report.read_text()
    assert table == capsys.readouterr().out
    assert table.splitlines()[0] == '#### Largest stack frames, sdk1-h1'
    assert '| `<vana::screens::workout::View>::new` | 10,992 | 12,000 | `*` |' in table
    assert 'reset_card' not in table


def test_a_frame_over_budget_fails_naming_it_and_the_command_for_new_budgets(tmp_path, objdump, capsys):
    assert run(tmp_path, objdump, BUDGETS.replace('= 1296', '= 200')) == 1
    err = capsys.readouterr().err
    assert 'Stack frame over budget: <vana::screens::workout::View>::prepare is 272 B, budget 200 B' in err
    assert f'mise run stack-frames {tmp_path / "cycling-os"} --variant sdk1-h1 --suggest' in err


def test_an_unnamed_frame_over_the_catch_all_fails(tmp_path, objdump, capsys):
    assert run(tmp_path, objdump, BUDGETS.replace('= 12000', '= 4000')) == 1
    assert 'View>::new is 10,992 B, budget 4,000 B (rule "*"' in capsys.readouterr().err


def test_a_rule_that_matches_nothing_warns_in_the_table_without_failing(tmp_path, objdump):
    report = tmp_path / 'stack-frames.md'
    stale = BUDGETS.replace('View>::prepare"', 'View>::prepare_page"')
    assert run(tmp_path, objdump, stale, '--report', str(report)) == 0
    assert 'WARNING: stack frame rule "<vana::screens::workout::View>::prepare_page"' in report.read_text()


def test_suggest_resets_named_budgets_and_never_raises_the_catch_all(tmp_path, objdump, capsys):
    budgets = BUDGETS.replace('= 12000', '= 4000').replace('View>::prepare"', 'View>::prepare_page"')
    assert run(tmp_path, objdump, budgets, '--suggest') == 0
    out, err = capsys.readouterr()
    assert out == (
        '[sdk1-h1]\n'
        f'"vana::screens::workout::fill::<*>" = {0x1C20 + 1024}\n'
        f'"<vana::screens::workout::View>::new" = {0x2AF0 + 1100}\n'
        '"*" = 4000\n'
    )
    assert 'Dropped: stack frame rule "<vana::screens::workout::View>::prepare_page"' in err


def test_a_missing_objdump_says_where_to_run_instead_of_a_traceback(tmp_path, capsys):
    with pytest.raises(SystemExit) as exit:
        run(tmp_path, tmp_path / 'no-objdump', BUDGETS)
    assert exit.value.code == 2
    assert 'run inside the dev shell (nix develop)' in capsys.readouterr().err
