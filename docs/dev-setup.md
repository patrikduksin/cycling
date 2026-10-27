# Development machines

Any x86_64 or aarch64 Linux machine with Nix becomes a cycling development
machine. [flake.nix](../flake.nix) and its lock own every tool version, and the
dev shell's `cycling` command runs every task
([cli.py](../tools/devtools/src/cycling_devtools/cli.py); `cycling` alone lists
them). Agents work on a fast remote machine (omni-devbox) while the C606 stays
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
`cycling verify --fast` once fills the compiler cache. Claude Code sessions
enter the shell through the [SessionStart hook](../.claude/hooks/dev-shell.sh),
which also keeps it from Nix garbage collection (`.local/dev-shell`).

What differs between machines belongs to the machine, not the repository: set
it in the environment, from the machine's own configuration. When the C606 is
on another machine, `CYCLING_DEVICE_HOST` names its ssh alias; omni-devbox sets
`CYCLING_DEVICE_HOST=c606-host` in its NixOS configuration.

Claude Code installs the pstack plugin the first time it opens a trusted
checkout: [.claude/settings.json](../.claude/settings.json) names its
marketplace and enables it.

## What the shell provides

- Host Rust 1.98.1 with clippy, rustfmt and rust-src (rust-overlay).
- The Xtensa toolchain ([nix/esp-rust.nix](../nix/esp-rust.nix)): esp-rs Rust
  1.97.0.0 with rust-src and Espressif's xtensa-esp-elf GCC, pinned by upstream
  release digests in place of espup. `CYCLING_ESP_TOOLCHAIN` points at it;
  `build.sh` puts it first on `PATH` for the firmware build only.
- Python 3.14 for uv (`UV_PYTHON`, downloads off), espflash, sccache, cargo-nextest, ffmpeg,
  gh, rsync and ssh. On NixOS, `LD_LIBRARY_PATH` carries what manylinux wheels
  and the live simulator window load.

`nix flake check` builds the whole environment; `nix flake update` moves every
pin at once and belongs in its own PR with `cycling verify` results.

## Fast builds

- `RUSTC_WRAPPER=sccache` with one cache per machine (`~/.cache/sccache`,
  50 GB), shared by every worktree: a new worktree compiles dependencies from
  cache. Override `SCCACHE_DIR` or `SCCACHE_CACHE_SIZE` in the environment.
- Target dirs stay per worktree, so parallel agents never queue on one cargo
  lock. `CYCLING_FW_TARGET_ROOT` moves the firmware target dirs elsewhere.
- Firmware always builds release (fat LTO). A lighter profile was measured and
  dropped: Xtensa runtime assembly needs one codegen unit and LTO, and thin
  LTO saved nothing over warm release rebuilds (about 20 s).
- `cycling verify` runs test, check, simulate and all four firmware builds in
  parallel, then harness-virtual alone, and prints one table; `--fast` is the
  inner-loop subset, `--pr N` posts the table on a PR. Logs go to
  `.local/verify/<commit>/`.

## A device on another machine

The device host needs this repository's primary checkout at
`~/projects/cycling` (override with `CYCLING_DEVICE_CHECKOUT`), set up as above,
and the ssh alias reachable without prompts. Machines reach each other over
Tailscale SSH: run `tailscale set --ssh` on the device host, and give the
tailnet policy an `accept` rule for your own devices, so agents never face the
browser re-check of the default `check` rule:

```jsonc
"ssh": [
  { "action": "accept", "src": ["autogroup:member"], "dst": ["autogroup:self"],
    "users": ["<device host user>", "<agent machine user>"] },
  // the default check rule, which still covers root
]
```

omni-devbox's `~/.ssh/config` names the laptop `c606-host` (its MagicDNS
name and user, no keys).

`cycling device <task> [args]` snapshots this checkout (uncommitted and
untracked files included) into a worktree of the host's primary checkout under
`.local/remote/`, so the host's USB lock, backup and safety checks apply as
usual. `device flash` builds here and sends only the image to `flash-image`.
The task's `.local` evidence is copied back to the same paths here. The
[remote device skill](../.agents/skills/remote-device/SKILL.md) owns the
procedure for agents.
