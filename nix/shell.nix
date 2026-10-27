# The one development environment: every tool a task needs, at the version
# flake.lock pins, and `cycling`, which runs the tasks.
{
  pkgs,
  lib,
  esp-rust,
}:
let
  rust = pkgs.rust-bin.stable."1.98.1".default.override {
    extensions = [
      "rust-src"
      "clippy"
      "rustfmt"
    ];
  };
  python = pkgs.python314;
  # Libraries that manylinux wheels (numpy, osmium) and the live simulator
  # window (minifb) load at run time. NixOS has no global /usr/lib for them.
  runtimeLibs = with pkgs; [
    stdenv.cc.cc.lib
    zlib
    libxkbcommon
    wayland
    libx11
    libxcursor
    libxrandr
    libxi
  ];
  # `cycling <task>` from anywhere in a checkout, in the locked devtools
  # environment; tools/devtools/src/cycling_devtools/cli.py owns the tasks.
  cycling = pkgs.writeShellScriptBin "cycling" ''
    root=$(git rev-parse --show-toplevel) || exit
    exec uv run --project "$root/tools/devtools" --locked python -m cycling_devtools.cli "$@"
  '';
in
pkgs.mkShell {
  packages = with pkgs; [
    rust
    cargo-nextest
    python
    uv
    cycling
    espflash
    sccache
    ffmpeg-headless
    pkg-config
    git
    gh
    jq
    rsync
    openssh
  ];

  env = {
    # build.sh puts this toolchain first only for the firmware build, so host
    # cargo stays the rust-overlay toolchain above.
    CYCLING_ESP_TOOLCHAIN = "${esp-rust}";
    UV_PYTHON = "${python}/bin/python3";
    UV_PYTHON_DOWNLOADS = "never";
    # This Python, even where the machine prefers uv-managed ones (omni-devbox
    # sets only-managed): otherwise every uv run recreates the venv, and
    # parallel lanes delete modules under each other.
    UV_PYTHON_PREFERENCE = "only-system";
    RUSTC_WRAPPER = "sccache";
    LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
  };

  # Settings that are never committed: one file for every checkout on this
  # machine (CYCLING_DEVICE_HOST), then one for this checkout. Then one sccache
  # per machine, shared by every worktree, unless those files set otherwise.
  shellHook = ''
    for env in "''${XDG_CONFIG_HOME:-$HOME/.config}/cycling/env.sh" "$(git rev-parse --show-toplevel 2>/dev/null)/.local/env.sh"; do
      if [ -f "$env" ]; then . "$env"; fi
    done
    export SCCACHE_DIR="''${SCCACHE_DIR:-''${XDG_CACHE_HOME:-$HOME/.cache}/sccache}"
    export SCCACHE_CACHE_SIZE="''${SCCACHE_CACHE_SIZE:-50G}"
    unset PYTHONPATH
  '';
}
