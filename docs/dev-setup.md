# Development machines

Any x86_64 or aarch64 Linux machine with Nix becomes a cycling development
machine. [flake.nix](../flake.nix) and its lock own every tool version, and the
dev shell's `cycling` command runs every task
([cli.py](../tools/devtools/src/cycling_devtools/cli.py); `cycling` alone lists
them). Agents work on a fast remote machine (fleet-hub) while the C606 stays
plugged into another one; only the image and text cross the network.

## Set up a machine

The machine needs Nix with flakes; the repository provides everything else.

```sh
git clone https://github.com/patrikduksin/cycling && cd cycling
nix develop
```

`nix develop` enters the dev shell; `nix develop -c <command>` runs one command
in it. The first shell downloads about 1 GB (about 4 GB in the Nix store, most
of it the two Rust toolchains); later ones start in about 2 s.
`cycling verify --all` once fills the compiler cache. Claude Code sessions
enter the shell through the [SessionStart hook](../.claude/hooks/dev-shell.sh),
which also keeps it from Nix garbage collection (`.local/dev-shell`).

What differs between machines belongs to the machine, not the repository: set
it in the environment, from the machine's own configuration. When the C606 is
on another machine, `CYCLING_DEVICE_HOST` names it as `user@<tailnet name>`;
fleet-hub sets `CYCLING_DEVICE_HOST=patrikduksin@patrikduksin` in its NixOS
configuration.

Claude Code installs the pstack plugin the first time it opens a trusted
checkout: [.claude/settings.json](../.claude/settings.json) names its
marketplace and enables it.

## What the shell provides

- Host Rust 1.100.0 with clippy, rustfmt and rust-src (rust-overlay).
- The Xtensa toolchain ([nix/esp-rust.nix](../nix/esp-rust.nix)): esp-rs Rust
  1.100.0.0 with rust-src and Espressif's xtensa-esp-elf GCC, pinned by upstream
  release digests in place of espup. `CYCLING_ESP_TOOLCHAIN` points at it;
  `build.sh` puts it first on `PATH` for the firmware build only.
- Python 3.14 for uv (`UV_PYTHON`, downloads off), espflash, sccache, cargo-nextest, ffmpeg,
  gh, rsync and ssh. On NixOS, `LD_LIBRARY_PATH` carries what manylinux wheels
  and the live simulator window load.

`nix flake check` builds the whole environment; `nix flake update` moves every
pin at once and belongs in its own PR with `cycling verify` results.

## Fast builds

- `RUSTC_WRAPPER=sccache` with one cache per machine (`~/.cache/sccache`,
  50 GB), shared by every worktree. Override `SCCACHE_DIR` or
  `SCCACHE_CACHE_SIZE` in the environment. It caches library dependencies
  only. A new worktree still compiles every workspace crate, build script,
  proc macro and test binary. It also compiles crates that `include!` a build
  script's output (serde, esp-hal) and the crates above them, because the
  worktree's `OUT_DIR` path is part of their key.
- sccache keys each compile on the working directory and on every `CARGO_*`
  variable. Pass `--target-dir` to cargo: an exported absolute
  `CARGO_TARGET_DIR` stops every other worktree from reusing those builds.
- Target dirs stay per worktree, so parallel agents never queue on one cargo
  lock. `CYCLING_FW_TARGET_ROOT` moves the firmware target dirs elsewhere.
- Firmware always builds release with fat LTO, because the Xtensa runtime
  assembly needs one codegen unit and LTO.
- The firmware compiles with a placeholder commit, which `build.sh` replaces
  in the linked ELF, so a commit that changes no firmware input rebuilds
  nothing.

## How verify certifies a tree

The [verify skill](../.agents/skills/verify/SKILL.md) says when to run
`cycling verify` and what comes after it. This section says how it works.

- It hashes the worktree as it is, untracked files included, and runs the
  gates in the worktree. After each gate, and once more before it reports, it
  hashes the tree again and compares every file's change time. If anything
  differs, an edit that was undone included, it stops the gates that are still
  running, names the files, and certifies nothing. A run that is interrupted
  or terminated stops its gates the same way, and `cycling sim-compare` stops
  the cargo runs it started.
- `RULES` in `tools/devtools/src/cycling_devtools/lanes.py` maps the paths
  changed since the merge-base with main to gates, and a path no rule matches
  selects every gate; `--all` selects them regardless. A workspace crate
  selects the firmware build when the package of a `devices/*/device.toml`
  depends on it. The table names the gates the paths did not select.
- The selected gates run in parallel lanes: the firmware build, the Rust tests
  then simulate, the Trouble Host tests, the Python tests, the web tests and
  checks, clippy and rustfmt, the other checks, and the screens. When every
  lane passes, harness-virtual runs alone. A `tests-passed` file appears in the log
  directory, `.local/verify/<tree>/`, once every selected test gate passed.
- The screens lane is `cycling sim-compare --base <merge-base>`: it runs the
  ride scenarios once, recorded, and compares them with the merge-base's. The
  test lane's nextest profile (`without-ride-scenarios`, in
  `.config/nextest.toml`) leaves the scenarios out. `--pr N` posts the table
  and the changed screens on a PR, with a video of each new scenario, and
  refuses a tree that is not its commit. `--video` adds before and after
  videos of changed scenarios.
- Each gate that passes is recorded for 30 days in the primary checkout's
  `.local/verify/results`. The key is the tree hash, the toolchain, the gate
  and its command, and the settings in the environment that change a build
  (`CYCLING_*`, `CARGO_BUILD_*`, `CARGO_PROFILE_*`, `CARGO_TARGET_*` except
  `CARGO_TARGET_DIR`, `RUSTFLAGS`, `RUSTDOCFLAGS`, their `CARGO_ENCODED_`
  forms, and `NEXTEST_PROFILE`). No worktree runs that gate on that tree again
  (`cached` in the table); `--fresh` runs it anyway.
- A verify with gates to run takes one of the machine's slots first, one per
  ten cores (`tools/devtools/src/cycling_devtools/admission.py`), and waits
  when none is free. Its time in the table includes the wait and names it. It
  refuses to start with under 20 GB free, and checks again once it has its
  slot.
- `--deep` runs tier 2 alone, at nice 19 and without a slot:
  `cycling fuzz --fixed`, `cycling fuzz-planted`, and the course Follower's
  large `explore` searches (`#[ignore]`d in
  `apps/vana/src/ride/course/follow/tests/explore/deep.rs`, so tier 1 runs only
  its small ones).
- nextest's `slow` group (`.config/nextest.toml`) holds the tests that
  `cycling quick` leaves out with `-E 'not group(slow)'`: a test over 5 s on
  its own, and every share of a sweep. `cycling verify` runs them all. Split
  a long VANA sweep into eight tests with `split_in_eight!`
  (`apps/vana/src/testing.rs`), so verify runs its shares side by side.

## How a merge lands

- `cycling merge <pr>` takes its turn on a lock in the primary checkout,
  squashes the PR onto main, and verifies that tree unless it already passed.
  It moves the PR's branch to the squash, posts the `cycling/verify` status
  that main requires on it, waits until GitHub shows it as the PR's head, and
  only then fast-forwards main to it, so GitHub marks the PR merged. If GitHub
  does not show the head within a minute, or main has moved or refuses the
  push, it puts the branch back at its head and says so. It deletes the PR's
  branch once GitHub shows the PR merged.
- No worktree has the squash checked out, so the merge verifies it in the
  machine's one merge checkout, `.local/merge/verify` in the primary checkout:
  a locked worktree with its own target dirs, which `cycling clean` leaves
  alone. `--prepare` squashes and verifies only, in the same queue.
- The merge refuses a PR whose base is not main, and one without a review
  PASS on its current patch. `cycling review patch --pr <n>` prints that
  patch, `git patch-id --verbatim` of what `gh pr diff` shows: the PR's head
  against its merge-base with the PR's base. `cycling review record --patch`
  refuses a verdict on any other. A PASS on a stacked PR therefore covers its
  own changes only, and it still holds after the parent lands and the PR is
  rebased onto main, if the rebased diff is the one that was reviewed.
- After a merge, two jobs run on the merged commit in the background at
  `nice 19`. `cycling sim-compare --fill` caches its screens as the base of
  later comparisons, with its log in `.local/merge/fill.log`. The slow tier
  runs in `.local/merge/slow-tier` of the primary checkout and comments on
  the merged PR if it fails or cannot run. The slow tier is
  `cycling test-slow`, every web test with `CYCLING_SLOW=1`: the tests verify
  leaves out. It builds no Rust. One runs at a time, and a run that finds
  main past its commit leaves the work to the newer merge's run. With under
  10 GB free it does not start, and says so on the PR. Its log is
  `.local/merge/slow-tier.log`.
- `cycling playbook-audit --session ID` prints a session digest for the
  auditor brief in `docs/agents/playbook-auditor.md`.

## Checks you can run on your own work

`cycling verify` certifies a PR. These checks find bugs earlier, or find the
ones its tier 1 gates do not reach. Run any of them at any time; none of them
posts anything.

- `cycling quick` runs nextest and clippy on the crates your diff touches,
  leaving out the `slow` group and the ride scenarios. When a device links one
  of those crates, it also runs `cycling check-firmware`, a firmware
  `cargo check`. It takes no slot and certifies nothing. It catches compile
  errors, lints and failing unit tests. Run it after every edit.
- An `explore` search (`apps/vana/src/testing.rs`) runs a state machine's real
  code breadth first from every starting state within small bounds, checks
  every named rule on every state and edge, and prints the shortest path to
  each broken rule. It catches an ordering or an interleaving that no
  hand-written test tried. Three machines have one: the ride recorder
  (`apps/vana/src/ride/recorder/explore.rs`), the ANT sensor scan
  (`apps/vana/src/sensors/scan/explore.rs`) and the course Follower
  (`apps/vana/src/ride/course/follow/tests/explore/`). The small searches run
  with the tests. After any change to `apps/vana/src/ride/recorder.rs`, its
  directory or `apps/vana/src/ride/journal.rs`, also run the recorder's
  ignored wide and full-batch searches. Their `#[ignore]` reasons give their
  time and memory.

  ```sh
  cargo nextest run --locked --no-default-features -p vana --lib --run-ignored only -E 'test(/^ride::recorder::explore::/)'
  ```

- The invariants are rules that must hold after every pass, such as "both
  journals riding" or "summary shown while riding". VANA states them in
  `apps/vana/src/app/invariants.rs` and
  `apps/vana/src/ride/recorder/invariants.rs`, compiled for tests and for the
  `invariants` feature and never for a device. The simulator checks them
  after every pass and panics on the first broken one, so every simulator
  test and every fuzz session checks them.
- `cycling fuzz` runs random sessions in the twin: courses, sensors, button
  presses, power cuts, torn writes and card refusals, judged after each step.
  It catches crashes, lost rides and broken invariants in combinations nobody
  wrote down. Run `cycling fuzz --minutes N` after a change to recording,
  storage, power or page flow. A finding prints its seed and the command that
  reproduces it. The [simulator README](../tools/simulator/README.md) gives
  the flags and what each check judges.
- `cycling verify --deep` runs tier 2: the fixed fuzz seeds, the planted-bug
  proof that the fuzzer still finds a bug, and the Follower's large `explore`
  searches. It takes no slot, runs at nice 19, and is not a PR gate. Run it
  when your change touches what it searches: course following, recording and
  storage, or the fuzzer.

## A device on another machine

The device host needs this repository's primary checkout at
`~/projects/cycling` (override with `CYCLING_DEVICE_CHECKOUT`), set up as above,
and `ssh <user>@<tailnet name>` working without prompts. Machines reach each
other over Tailscale SSH, set up fleet-wide in
[fleet-nix](https://github.com/patrikduksin/fleet-nix): no keys, no ssh aliases.

`cycling device <task> [args]` snapshots this checkout (uncommitted and
untracked files included) into a worktree of the host's primary checkout under
`.local/remote/`, so the host's USB lock, backup and safety checks apply as
usual. `device flash` builds here and sends only the image to `flash-image`.
The task's `.local` evidence is copied back to the same paths here. The
[remote device skill](../.agents/skills/remote-device/SKILL.md) owns the
procedure for agents.
