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
  worktree's `OUT_DIR` path is part of their key. The first `cycling verify`
  in a new worktree takes about 130 s, against about 80 s warm.
- sccache keys each compile on the working directory and on every `CARGO_*`
  variable. Pass `--target-dir` to cargo: an exported absolute
  `CARGO_TARGET_DIR` stops every other worktree from reusing those builds.
- Target dirs stay per worktree, so parallel agents never queue on one cargo
  lock. `CYCLING_FW_TARGET_ROOT` moves the firmware target dirs elsewhere.
- Firmware always builds release (fat LTO). A lighter profile was measured and
  dropped: Xtensa runtime assembly needs one codegen unit and LTO, and thin
  LTO saved nothing over warm release rebuilds. After a VANA edit a warm
  rebuild compiles vana (9 s), the c606-firmware lib (6 s) and the fat-LTO bin
  (30 s) one after another; packaging and the stack checks take 1 s.
- The firmware compiles with a placeholder commit, which `build.sh` replaces
  in the linked ELF, so a commit that changes no firmware input rebuilds
  nothing (about 2 s).
- `cycling verify` certifies a tree. It hashes the worktree as it is,
  untracked files included, and runs the gates in the worktree. After each
  gate, and once more before it reports, it hashes the tree again and compares
  every file's change time. If anything differs, an edit that was undone
  included, it stops the gates that are still running, names the files, and
  certifies nothing. A run that is interrupted or terminated stops its gates
  the same way, and `cycling sim-compare` stops the cargo runs it started. Do
  not edit while it runs. The table in `tools/devtools/src/cycling_devtools/lanes.py` maps
  the paths changed since the merge-base with main to gates, and a path it
  does not know selects every gate; `--all` selects them regardless. A
  workspace crate selects the firmware build when the package of a
  `devices/*/device.toml` depends on it. The firmware build, the tests, the
  checks, simulate and the screens run in parallel lanes, the firmware first,
  then harness-virtual alone. Each gate that passes is recorded in the primary
  checkout's `.local/verify/results` under the tree hash, the toolchain, the
  gate's definition and the settings in the environment that change a build
  (`CYCLING_*`, `RUSTFLAGS`, `CARGO_BUILD_*`, `CARGO_PROFILE_*`,
  `NEXTEST_PROFILE`), so no worktree runs it on that tree again (`cached`
  in the table; `--fresh` runs it anyway). The screens lane is
  `cycling sim-compare --base <merge-base>`: it runs the ride scenarios once,
  recorded, and compares them with the merge-base's, and the
  test lane's nextest profile (`without-ride-scenarios`, in
  `.config/nextest.toml`) leaves them out. `--pr N` posts the table and the
  changed screens on a PR, and refuses a tree that is not its commit;
  `--video` adds before and after videos of changed scenarios. Logs go to `.local/verify/<tree>/`, and a `tests-passed` file
  appears there when the test gates have passed. `--deep` runs tier 2 alone,
  at nice 19 and without a slot: `cycling fuzz --fixed` and
  `cycling fuzz-planted`.
- A verify with gates to run takes one of the machine's slots first, one per
  ten cores (`tools/devtools/src/cycling_devtools/admission.py`), and waits
  when none is free. Its time in the table includes the wait and names it. It
  refuses to start with under 20 GB free, and checks again once it has its
  slot.
- `cycling quick` is the inner loop: nextest without the `slow` group and the
  ride scenarios, clippy on the crates the diff touches, and
  `cycling check-firmware` when a device links one of them. It takes no slot
  and certifies nothing.
- nextest's `slow` group (`.config/nextest.toml`) holds the tests that
  `cycling quick` leaves out with `-E 'not group(slow)'`: a test over 5 s on
  its own, and every share of a sweep. `cycling verify` runs them all. Split
  a long VANA sweep into eight tests with `split_in_eight!`
  (`apps/vana/src/testing.rs`), so verify runs its shares side by side. The
  48 sweep shares cost `cycling quick` 200 of its 265 CPU-s after a VANA
  edit.
- `cycling playbook-audit --session ID` prints a session digest for the
  auditor brief in `docs/agents/playbook-auditor.md`.

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
