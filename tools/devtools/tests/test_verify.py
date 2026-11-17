import ast
import os
import signal
import subprocess
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from cycling_devtools import admission, lanes, quick, runner, tree, verify, workspace
from verify_fixture import commit, gate_runs, git


def select(*paths):
    return lanes.select(paths, workspace.ROOT)


def test_a_docs_change_selects_no_gate():
    assert select('docs/ui.md', 'README.md') == frozenset()


def test_rust_under_apps_selects_the_firmware_the_host_tests_and_pytest_but_not_the_web():
    selected = select('apps/vana/src/app.rs')
    assert {'build', 'test-rust', 'check-rust', 'screens', 'test-python', 'harness-virtual'} <= selected
    assert not {'test-web', 'check-web'} & selected


def test_the_simulator_selects_pytest_but_not_the_firmware():
    selected = select('tools/simulator/src/session.rs')
    assert 'test-python' in selected
    assert 'build' not in selected


def test_devtools_select_the_firmware_but_not_the_rust_tests():
    selected = select('tools/devtools/src/cycling_devtools/rides/export.py')
    assert {'build', 'test-python', 'check-python'} <= selected
    assert 'test-rust' not in selected


def test_a_path_no_rule_knows_selects_every_gate():
    assert select('docs/ui.md', 'firmware-next/src/main.rs') == lanes.EVERY


def imported_devtools_modules(module):
    source = Path(verify.__file__).with_name(f'{module}.py').read_text()
    found = set()
    for node in ast.walk(ast.parse(source)):
        if isinstance(node, ast.ImportFrom) and node.module == 'cycling_devtools':
            found.update(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and (node.module or '').startswith('cycling_devtools.'):
            found.add(node.module.split('.')[1])
    return found


def test_verify_and_every_module_it_imports_select_every_gate():
    modules, todo = set(), ['verify']
    while todo:
        module = todo.pop()
        if module not in modules:
            modules.add(module)
            todo += imported_devtools_modules(module)
    assert set(lanes.VERIFY_MODULES) == modules | {'cli'}
    for module in lanes.VERIFY_MODULES:
        assert select(f'tools/devtools/src/cycling_devtools/{module}.py') == lanes.EVERY


def test_the_skill_a_rust_test_reads_selects_the_rust_tests():
    assert 'test-rust' in select('.agents/skills/ride-trace/SKILL.md')
    assert 'test-rust' not in select('.agents/skills/verify/SKILL.md')


def test_the_cargo_configuration_selects_the_vendored_host_tests_too():
    assert {'build', 'test-rust', 'test-trouble-host'} <= select('.cargo/config.toml')


def test_every_rule_names_known_gates_and_every_gate_has_a_rule():
    named = frozenset().union(*(gates for _, gates in lanes.RULES))
    assert named == lanes.EVERY


def test_the_tree_hash_covers_untracked_files_but_not_ignored_ones(checkout):
    clean = tree.tree_hash(checkout)
    assert clean == git('rev-parse', 'HEAD^{tree}', cwd=checkout)
    (checkout / '.local').mkdir()
    (checkout / '.local' / 'notes').write_text('private')
    assert tree.tree_hash(checkout) == clean
    (checkout / 'src' / 'new.txt').write_text('new')
    assert tree.tree_hash(checkout) != clean


def test_hashing_the_tree_leaves_the_worktree_and_its_index_alone(checkout):
    (checkout / 'src' / 'new.txt').write_text('new')
    (checkout / 'src' / 'a.txt').write_text('edited')
    before = git('status', '--porcelain', cwd=checkout)
    tree.tree_hash(checkout)
    assert git('status', '--porcelain', cwd=checkout) == before
    assert '?? src/new.txt' in before


def test_the_firmware_lane_starts_first_and_builds_beside_the_flashed_image():
    assert lanes.LANES[0] == (lanes.BUILD,)
    assert dict(lanes.BUILD.env)['CYCLING_IMAGE_DIR'] != '.local'


def test_the_screens_gate_posts_on_the_pull_request_and_the_tests_leave_the_scenarios_to_it():
    gates = {gate.name: gate for lane in lanes.LANES for gate in lane}
    posting = verify.Request(workspace.ROOT, 'base', pr=7, video=True)
    assert posting.command(gates['screens']) == 'cycling sim-compare --base base --pr 7 --video'
    assert verify.Request(workspace.ROOT, 'base').command(gates['screens']) == 'cycling sim-compare --base base'
    assert posting.command(gates['test-rust']) == 'cycling test-rust'
    assert dict(gates['test-rust'].env)['NEXTEST_PROFILE'] == 'without-ride-scenarios'


def test_a_failed_gate_stops_its_lane_only(checkout, monkeypatch):
    first, second, other = (
        lanes.Gate(name, command) for name, command in (('first', 'exit 3'), ('second', 'true'), ('other', 'true'))
    )
    monkeypatch.setattr(lanes, 'LANES', ((first, second), (other,)))
    monkeypatch.setattr(lanes, 'EVERY', frozenset({'first', 'second', 'other'}))
    outcome = verify.verify(request(checkout, everything=True))
    assert not outcome.ok
    assert '| `first` | ❌ |' in outcome.report and '| `other` | ✅ |' in outcome.report
    assert '`second`' not in outcome.report
    assert '$ exit 3' in outcome.failures


def request(root, **options):
    return verify.Request(root, git('rev-parse', 'origin/main', cwd=root), **options)


def test_a_tree_that_passed_is_not_run_again_and_says_so(checkout):
    (checkout / 'src' / 'new.txt').write_text('new')
    first = verify.verify(request(checkout))
    assert first.ok and 'cached' not in first.report
    again = verify.verify(request(checkout))
    assert again.ok and '| `probe` | ✅ | cached' in again.report
    assert gate_runs(checkout) == 1
    assert verify.verify(request(checkout, fresh=True)).ok
    assert gate_runs(checkout) == 2


def test_a_pass_that_posted_on_a_pull_request_stands_for_a_run_that_posts_nothing_but_not_the_reverse(
    checkout, monkeypatch
):
    screens = lanes.Gate('probe', 'echo ran >> "$GATE_RUNS"; true', reports=True)
    monkeypatch.setattr(lanes, 'LANES', ((screens,),))
    commit(checkout, 'Change', **{'src/b.txt': 'b'})
    assert verify.verify(request(checkout)).ok
    assert verify.verify(request(checkout, pr=7)).ok
    assert gate_runs(checkout) == 2
    assert verify.verify(request(checkout, pr=7)).ok
    assert verify.verify(request(checkout)).ok
    assert gate_runs(checkout) == 2
    commit(checkout, 'Again', **{'src/b.txt': 'c'})
    assert verify.verify(request(checkout, pr=7)).ok
    assert verify.verify(request(checkout)).ok
    assert gate_runs(checkout) == 3


def test_a_failure_is_reported_with_its_log_and_never_cached(checkout):
    (checkout / 'broken').write_text('')
    for _ in range(2):
        outcome = verify.verify(request(checkout))
        assert not outcome.ok
        assert '| `probe` | ❌ |' in outcome.report
        assert '--- probe' in outcome.failures
    assert gate_runs(checkout) == 2


def test_an_untracked_file_is_verified_and_changes_the_result(checkout):
    (checkout / 'src' / 'new.txt').write_text('new')
    assert verify.verify(request(checkout)).ok
    (checkout / 'broken').write_text('')
    assert not verify.verify(request(checkout)).ok


def test_a_change_that_selects_no_gate_passes_without_running_one(checkout):
    commit(checkout, 'Docs', **{'docs/ui.md': 'words'})
    outcome = verify.verify(request(checkout))
    assert outcome.ok
    assert 'Not selected by the changed paths: `probe`.' in outcome.report
    assert gate_runs(checkout) == 0
    assert verify.verify(request(checkout, everything=True)).ok
    assert gate_runs(checkout) == 1


def test_an_edit_while_the_gates_run_stops_the_run_and_certifies_nothing(checkout, monkeypatch):
    editing = lanes.Gate('probe', 'echo ran >> "$GATE_RUNS"; echo edited > src/a.txt; echo new > src/new.txt')
    monkeypatch.setattr(lanes, 'LANES', ((editing,),))
    outcome = verify.verify(request(checkout, everything=True))
    assert not outcome.ok
    assert 'the tree changed while its gates ran.\n\nsrc/a.txt\nsrc/new.txt' in outcome.report
    assert not verify.verify(request(checkout, everything=True)).ok
    assert gate_runs(checkout) == 2


def test_an_edit_that_was_undone_while_the_gates_ran_stops_the_run_too(checkout, monkeypatch):
    rewriting = lanes.Gate('probe', 'printf "a\\n" > src/a.txt')
    monkeypatch.setattr(lanes, 'LANES', ((rewriting,),))
    before = tree.tree_hash(checkout)
    outcome = verify.verify(request(checkout, everything=True))
    assert tree.tree_hash(checkout) == before
    assert not outcome.ok
    assert outcome.report.endswith('\n\nsrc/a.txt')


def test_an_edit_stops_the_gates_that_are_still_running(checkout, monkeypatch):
    editing, long = lanes.Gate('editing', 'echo edited > src/a.txt'), lanes.Gate('long', 'sleep 60 & wait')
    monkeypatch.setattr(lanes, 'LANES', ((long,), (editing,)))
    monkeypatch.setattr(lanes, 'EVERY', frozenset({'editing', 'long'}))
    started = time.monotonic()
    assert not verify.verify(request(checkout, everything=True)).ok
    assert time.monotonic() - started < 30


def test_an_edit_keeps_a_gate_that_was_about_to_start_from_starting(checkout, monkeypatch):
    editing, long = lanes.Gate('editing', 'echo edited > src/a.txt'), lanes.Gate('long', 'sleep 60 & wait')
    monkeypatch.setattr(lanes, 'LANES', ((long,), (editing,)))
    monkeypatch.setattr(lanes, 'EVERY', frozenset({'editing', 'long'}))
    frame_report = verify.frame_report

    def late_for_long(gate, root):
        if gate is long:
            time.sleep(2)
        return frame_report(gate, root)

    monkeypatch.setattr(verify, 'frame_report', late_for_long)
    started = time.monotonic()
    assert not verify.verify(request(checkout, everything=True)).ok
    assert time.monotonic() - started < 30


def test_a_run_waits_for_a_free_slot_and_reports_the_wait_but_a_cached_tree_does_not_queue(checkout, monkeypatch):
    monkeypatch.setattr(admission, 'slots', lambda: 1)
    monkeypatch.setattr(admission, 'POLL_SECONDS', 0.05)
    (checkout / 'src' / 'new.txt').write_text('new')
    with ThreadPoolExecutor(1) as pool:
        with admission.admitted():
            waiting = pool.submit(verify.verify, request(checkout))
            time.sleep(2.5)
            assert gate_runs(checkout) == 0
        outcome = waiting.result()
    assert outcome.ok and ' s queued), ' in outcome.report
    with admission.admitted():
        again = verify.verify(request(checkout))
    assert again.ok and 'queued' not in again.report


def test_a_run_that_waited_while_another_passed_its_tree_runs_nothing(checkout, monkeypatch):
    monkeypatch.setattr(admission, 'slots', lambda: 1)
    monkeypatch.setattr(admission, 'POLL_SECONDS', 0.05)
    (checkout / 'src' / 'new.txt').write_text('new')
    with ThreadPoolExecutor(2) as pool:
        with admission.admitted():
            waiting = [pool.submit(verify.verify, request(checkout)) for _ in range(2)]
            time.sleep(1.5)
        first, second = sorted((run.result() for run in waiting), key=lambda outcome: 'cached' in outcome.report)
    assert first.ok and second.ok
    assert 'cached' not in first.report and '| `probe` | ✅ | cached' in second.report
    assert gate_runs(checkout) == 1


def test_a_run_is_refused_when_the_disk_is_nearly_full_unless_every_gate_is_cached(checkout, monkeypatch):
    (checkout / 'src' / 'new.txt').write_text('new')
    assert verify.verify(request(checkout)).ok
    monkeypatch.setattr(admission.clean, 'free_gb', lambda path: 19.0)
    assert verify.verify(request(checkout)).ok
    with pytest.raises(SystemExit, match='19 GB free, under 20 GB'):
        verify.verify(request(checkout, fresh=True))
    assert gate_runs(checkout) == 1


def test_deep_runs_tier_two_alone_and_holds_no_slot(checkout, monkeypatch):
    monkeypatch.setattr(lanes, 'DEEP', (lanes.Gate('fuzz', 'echo ran >> "$GATE_RUNS"'),))
    monkeypatch.setattr(admission, 'slots', lambda: 1)
    with admission.admitted():
        outcome = verify.verify(request(checkout, deep=True))
    assert outcome.ok
    assert outcome.report.startswith('### `cycling verify --deep`')
    assert '| `fuzz` | ✅ |' in outcome.report and 'probe' not in outcome.report
    assert gate_runs(checkout) == 1
    assert not list((checkout / '.local' / 'verify').glob(f'*/{verify.TESTS_PASSED}'))


def test_the_tests_passed_marker_appears_only_when_the_test_gates_pass(checkout):
    (checkout / 'src' / 'new.txt').write_text('new')
    verify.verify(request(checkout))
    markers = list((checkout / '.local' / 'verify').glob(f'*/{verify.TESTS_PASSED}'))
    assert len(markers) == 1
    (checkout / 'broken').write_text('')
    verify.verify(request(checkout))
    assert list((checkout / '.local' / 'verify').glob(f'*/{verify.TESTS_PASSED}')) == markers


def test_quick_checks_the_crates_the_diff_touches_and_the_firmware_when_a_device_links_one():
    firmware = lanes.firmware_crates(workspace.ROOT)
    crates = quick.touched(['apps/vana/src/app.rs', 'docs/ui.md', 'tools/simulator/src/session.rs'], workspace.ROOT)
    assert crates == {'apps/vana': 'vana', 'tools/simulator': 'shell-simulator'}
    assert [gate.name for gate in quick.gates(crates, firmware)] == ['test', 'clippy', 'check-firmware']
    simulator = {'tools/simulator': 'shell-simulator'}
    assert [gate.name for gate in quick.gates(simulator, firmware)] == ['test', 'clippy']
    assert "-p vana -p shell-simulator -E 'not group(slow)'" in quick.gates(crates, firmware)[0].command
    assert quick.gates({}, firmware) == []


def test_quick_leaves_the_ride_scenarios_to_verify():
    test = quick.gates({'tools/simulator': 'shell-simulator'}, set())[0]
    assert dict(test.env)['NEXTEST_PROFILE'] == 'without-ride-scenarios'


def test_the_firmware_is_built_for_the_crates_a_device_manifest_links_and_not_for_the_others():
    assert {'devices/c606', 'apps/vana', 'packages/shell'} <= lanes.firmware_crates(workspace.ROOT)
    assert 'tools/simulator' not in lanes.firmware_crates(workspace.ROOT)
    assert 'build' in select('packages/shell/src/shell.rs')
    assert 'build' in select('devices/c606/device.toml')
    assert 'build' in select('devices/build.sh')


def write(root, files):
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)


def test_the_firmware_follows_target_build_and_workspace_dependencies_of_the_tree_it_is_asked_about(
    tmp_path, monkeypatch
):
    package = '[package]\nname = "{}"\n'
    on_target = (
        '[target.\'cfg(target_arch = "xtensa")\'.dependencies]\n'
        'renamed = { path = "../on-target", package = "on-target" }\n'
        '[dev-dependencies]\ntested-with = { path = "../tested-with" }\n'
    )
    write(
        tmp_path,
        {
            'Cargo.toml': '[workspace]\nmembers = ["dev", "on-target", "at-build", "shared", "tested-with", "tool"]\n'
            '[workspace.dependencies]\nshared = { path = "shared" }\n',
            'devices/unit/device.toml': '[build]\npackage = "unit-firmware"\n',
            'dev/Cargo.toml': package.format('unit-firmware') + on_target,
            'on-target/Cargo.toml': package.format('on-target')
            + '[build-dependencies]\nat-build = { path = "../at-build" }\n',
            'at-build/Cargo.toml': package.format('at-build') + '[dependencies]\nshared.workspace = true\n',
            'shared/Cargo.toml': package.format('shared'),
            'tested-with/Cargo.toml': package.format('tested-with'),
            'tool/Cargo.toml': package.format('tool'),
        },
    )
    assert lanes.firmware_crates(tmp_path) == {'dev', 'on-target', 'at-build', 'shared'}
    monkeypatch.setattr(lanes, 'RULES', (('shared/', lanes.NONE), ('tool/', lanes.NONE)))
    assert lanes.select(['shared/src/lib.rs'], tmp_path) == {'build'}
    assert lanes.select(['tool/src/lib.rs'], tmp_path) == lanes.NONE
    assert lanes.select(['shared/src/lib.rs'], workspace.ROOT) == lanes.NONE


def test_an_edit_in_the_second_the_index_was_written_changes_the_tree_hash(checkout):
    while time.time() % 1 > 0.5:
        time.sleep(0.05)
    (checkout / 'src' / 'a.txt').write_text('a\n')
    git('add', '-A', cwd=checkout)
    (checkout / 'src' / 'a.txt').write_text('b\n')
    time.sleep(1.1)
    assert tree.tree_hash(checkout) != git('rev-parse', 'HEAD^{tree}', cwd=checkout)


def test_a_pass_under_one_setting_of_the_build_does_not_stand_for_another(checkout, monkeypatch):
    (checkout / 'src' / 'new.txt').write_text('new')
    monkeypatch.setenv('CYCLING_MIN_STACK_BYTES', '0')
    assert verify.verify(request(checkout)).ok
    monkeypatch.setenv('EDITOR', 'ed')
    assert verify.verify(request(checkout)).ok
    assert gate_runs(checkout) == 1
    settings = (
        ('CYCLING_MIN_STACK_BYTES', '999999999'),
        ('RUSTFLAGS', '-Dwarnings'),
        ('CARGO_PROFILE_DEV_DEBUG', '0'),
        ('RUSTDOCFLAGS', '--invalid'),
        ('CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS', '-Dwarnings'),
    )
    for name, value in settings:
        monkeypatch.setenv(name, value)
        assert verify.verify(request(checkout)).ok
    assert gate_runs(checkout) == 6


def test_an_edit_during_a_run_that_found_every_gate_cached_is_not_certified(checkout, monkeypatch):
    (checkout / 'src' / 'new.txt').write_text('new')
    assert verify.verify(request(checkout)).ok
    cached_pass = tree.cached_pass

    def edited_after_the_lookup(key):
        (checkout / 'src' / 'a.txt').write_text('edited')
        return cached_pass(key)

    monkeypatch.setattr(tree, 'cached_pass', edited_after_the_lookup)
    outcome = verify.verify(request(checkout))
    assert not outcome.ok
    assert 'the tree changed while its gates ran.\n\nsrc/a.txt' in outcome.report


def test_an_edit_during_a_run_that_selects_no_gate_is_not_certified(checkout, monkeypatch):
    commit(checkout, 'Docs', **{'docs/ui.md': 'words'})
    plan_for = verify.plan_for

    def edited_after_the_plan(request, watch):
        (checkout / 'docs' / 'ui.md').write_text('more words')
        return plan_for(request, watch)

    monkeypatch.setattr(verify, 'plan_for', edited_after_the_plan)
    assert not verify.verify(request(checkout)).ok


def test_results_for_a_pull_request_are_refused_inside_the_run_when_the_tree_is_not_the_commit(checkout):
    commit(checkout, 'Change', **{'src/b.txt': 'b'})
    (checkout / 'src' / 'untracked.txt').write_text('new')
    with pytest.raises(SystemExit, match=r'differ from HEAD(.|\n)*\n\nsrc/untracked.txt'):
        verify.verify(request(checkout, pr=7))
    assert gate_runs(checkout) == 0


def alive(pid):
    try:
        return Path(f'/proc/{pid}/stat').read_text().rpartition(')')[2].split()[0] != 'Z'
    except OSError:
        return False


def gone(pid_file):
    pid = int(pid_file.read_text())
    deadline = time.monotonic() + 10
    while alive(pid) and time.monotonic() < deadline:
        time.sleep(0.05)
    return not alive(pid)


def started(pid_file):
    deadline = time.monotonic() + 20
    while not (running := pid_file.exists() and pid_file.read_text().strip()) and time.monotonic() < deadline:
        time.sleep(0.05)
    return running


LONG = 'sleep 300 & echo $! > "$PID_FILE"; wait'


@pytest.mark.parametrize('number', [signal.SIGTERM, signal.SIGHUP])
@pytest.mark.parametrize('lane', ['parallel', 'alone'])
def test_a_terminated_run_stops_its_gates_in_either_kind_of_lane(
    checkout, monkeypatch, tmp_path, restored_signals, number, lane
):
    runner.unwind_on_termination()
    monkeypatch.setenv('PID_FILE', str(tmp_path / 'pid'))
    long = (lanes.Gate('probe', LONG),)
    monkeypatch.setattr(lanes, 'LANES', (long,) if lane == 'parallel' else ())
    monkeypatch.setattr(lanes, 'ALONE', long if lane == 'alone' else ())
    threading.Thread(target=lambda: started(tmp_path / 'pid') and os.kill(os.getpid(), number)).start()
    with pytest.raises(SystemExit) as stopped:
        verify.verify(request(checkout, everything=True))
    assert stopped.value.code == 128 + number
    assert gone(tmp_path / 'pid')


def test_a_second_termination_signal_cannot_interrupt_the_unwinding(restored_signals):
    runner.unwind_on_termination()
    with pytest.raises(SystemExit):
        os.kill(os.getpid(), signal.SIGTERM)
    os.kill(os.getpid(), signal.SIGTERM)
    os.kill(os.getpid(), signal.SIGHUP)
    ignored = subprocess.run(['grep', 'SigIgn', '/proc/self/status'], capture_output=True, text=True).stdout
    assert not int(ignored.split()[1], 16) & (1 << (signal.SIGTERM - 1)), 'a gate started now still dies on SIGTERM'


def test_a_hangup_the_caller_ignores_stays_ignored(restored_signals):
    signal.signal(signal.SIGHUP, signal.SIG_IGN)
    runner.unwind_on_termination()
    assert signal.getsignal(signal.SIGHUP) is signal.SIG_IGN
    assert signal.getsignal(signal.SIGTERM) is not signal.SIG_IGN


def test_the_disk_is_checked_again_once_the_slot_is_held(checkout, monkeypatch):
    (checkout / 'src' / 'new.txt').write_text('new')
    free = iter([25.0, 19.0])
    monkeypatch.setattr(admission.clean, 'free_gb', lambda path: next(free))
    with pytest.raises(SystemExit, match='19 GB free, under 20 GB'):
        verify.verify(request(checkout))
    assert gate_runs(checkout) == 0


def test_the_gates_are_selected_from_the_tree_under_verification(checkout, monkeypatch, tmp_path):
    asked = []
    monkeypatch.setattr(lanes, 'firmware_crates', lambda root: asked.append(root) or ())
    elsewhere = tmp_path / 'elsewhere'
    git('worktree', 'add', '-q', '--detach', str(elsewhere), cwd=checkout)
    (elsewhere / 'src' / 'new.txt').write_text('new')
    assert verify.verify(request(elsewhere)).ok
    assert asked == [elsewhere]
