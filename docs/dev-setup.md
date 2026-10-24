# Development machines

Any x86_64 or aarch64 Linux machine becomes a cycling development machine with
one command. [flake.nix](../flake.nix) and its lock own every tool version; mise
only runs tasks. Agents work on a fast remote machine (omni-devbox) while the
C606 stays plugged into another one; only the image and text cross the network.

## Set up a machine

```sh
git clone https://github.com/patrikduksin/cycling && cd cycling
scripts/bootstrap.sh                               # this machine has the C606
scripts/bootstrap.sh --device-host c606-host      # the C606 is on c606-host
```

The script is idempotent. It installs Nix with flakes if missing, direnv with
nix-direnv, trusts this checkout and its T3 Code worktrees
(`~/.t3/worktrees/cycling`) so they need no `direnv allow`, builds the dev
shell and, with `--device-host`, records where device tasks go in
`~/.config/cycling/envrc`. `--warm` also runs `mise run verify --fast` to fill
the compiler cache. The first shell downloads about 1 GB (about 4 GB in the
Nix store, most of it the two Rust toolchains); later shells start from
nix-direnv's cache instantly.

Entering a checkout loads the shell through direnv. Without direnv, prefix
commands with `nix develop -c`. Claude Code sessions load it through the
[SessionStart hook](../.claude/hooks/dev-shell.sh), because agent shells are
not interactive and never run direnv's prompt hook.

## What the shell provides

- Host Rust 1.98.1 with clippy, rustfmt and rust-src (rust-overlay).
- The Xtensa toolchain ([nix/esp-rust.nix](../nix/esp-rust.nix)): esp-rs Rust
  1.97.0.0 with rust-src and Espressif's xtensa-esp-elf GCC, pinned by upstream
  release digests in place of espup. `CYCLING_ESP_TOOLCHAIN` points at it;
  `build.sh` puts it first on `PATH` for the firmware build only.
- Python 3.14 for uv (`UV_PYTHON`, downloads off), espflash, sccache, ffmpeg,
  gh, rsync and ssh. On NixOS, `LD_LIBRARY_PATH` carries what manylinux wheels
  and the live simulator window load.

`nix flake check` builds the whole environment; `nix flake update` moves every
pin at once and belongs in its own PR with `mise run verify` results.

## Fast builds

- `RUSTC_WRAPPER=sccache` with one cache per machine (`~/.cache/sccache`,
  50 GB), shared by every worktree: a new worktree compiles dependencies from
  cache. Override `SCCACHE_DIR` or `SCCACHE_CACHE_SIZE` in
  `~/.config/cycling/envrc` or `.envrc.local`.
- Target dirs stay per worktree, so parallel agents never queue on one cargo
  lock. `CYCLING_FW_TARGET_ROOT` moves the firmware target dirs elsewhere.
- Firmware always builds release (fat LTO). A lighter profile was measured and
  dropped: Xtensa runtime assembly needs one codegen unit and LTO, and thin
  LTO saved nothing over warm release rebuilds (about 20 s).
- `mise run verify` runs test, check, simulate and all four firmware builds in
  parallel, then harness-virtual alone, and prints one table; `--fast` is the
  inner-loop subset, `--pr N` posts the table on a PR. Logs go to
  `.local/verify/<commit>/`.

## A device on another machine

The device host needs this repository's primary checkout at
`~/projects/cycling` (override with `CYCLING_DEVICE_CHECKOUT`), bootstrapped,
and the ssh alias reachable without prompts. omni-devbox reaches the laptop as
`c606-host`: OpenSSH on the laptop listens only on its Tailscale address and
accepts only omni-devbox's key from omni-devbox's address. Tailscale SSH is off
there because the tailnet's `check` mode needs a browser.

`mise run device <task> [args]` snapshots this checkout (uncommitted and
untracked files included) into a worktree of the host's primary checkout under
`.local/remote/`, so the host's USB lock, backup and safety checks apply as
usual. `device flash` builds here and sends only the image to `flash-image`.
The task's `.local` evidence is copied back to the same paths here. The
[remote device skill](../.agents/skills/remote-device/SKILL.md) owns the
procedure for agents.
