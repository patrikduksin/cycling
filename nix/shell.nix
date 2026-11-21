# The one development environment: every tool a task needs, at the version
# flake.lock pins, and `cycling`, which runs the tasks.
{
  pkgs,
  lib,
  esp-rust,
}:
let
  rust = pkgs.rust-bin.stable."1.100.0".default.override {
    extensions = [
      "rust-src"
      "clippy"
      "rustfmt"
    ];
    # Script tiles: VANA's build compiles its WebAssembly guests.
    targets = [ "wasm32-unknown-unknown" ];
  };
  # The WebAssembly runtime script tiles run under (packages/wamr builds its C).
  wamr = pkgs.fetchFromGitHub {
    owner = "bytecodealliance";
    repo = "wasm-micro-runtime";
    rev = "WAMR-2.4.6";
    hash = "sha256-Lsa7P8QQVWB08UFIidPIsmaXZOtnDxE3C2MzW73Rwtg=";
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
    # Starting Python under load takes most of a second, and agents query
    # traces constantly.
    case "''${1:-}" in
      trace | replay) cd "$root" && exec bash tools/simulator/trace.sh "$@" ;;
    esac
    exec uv run --project "$root/tools/devtools" --locked python -m cycling_devtools.cli "$@"
  '';
  gatedGh = pkgs.writeShellScriptBin "gh" ''
    if root=$(git rev-parse --show-toplevel 2>/dev/null) && [ -x "$root/tools/pr-gates.sh" ]; then
      "$root/tools/pr-gates.sh" "$@" || exit
    fi
    exec ${pkgs.gh}/bin/gh "$@"
  '';
in
pkgs.mkShell {
  packages = with pkgs; [
    rust
    cargo-nextest
    python
    uv
    bun
    cycling
    espflash
    sccache
    ffmpeg-headless
    pkg-config
    git
    gatedGh
    jq
    rsync
    util-linux
    openssh
  ];

  env = {
    # build.sh puts this toolchain first only for the firmware build, so host
    # cargo stays the rust-overlay toolchain above.
    CYCLING_ESP_TOOLCHAIN = "${esp-rust}";
    CYCLING_WAMR = "${wamr}";
    # The firmware build puts the Xtensa rustc first on PATH; guests compile
    # for wasm32 with this one.
    CYCLING_WASM_RUSTC = "${rust}/bin/rustc";
    UV_PYTHON = "${python}/bin/python3";
    UV_PYTHON_DOWNLOADS = "never";
    # This Python, even where the machine prefers uv-managed ones (fleet-hub
    # sets only-managed): otherwise every uv run recreates the venv, and
    # parallel lanes delete modules under each other.
    UV_PYTHON_PREFERENCE = "only-system";
    RUSTC_WRAPPER = "sccache";
    LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
  };

  # One sccache per machine, shared by every worktree. Override either value
  # in the environment.
  shellHook = ''
    export SCCACHE_DIR="''${SCCACHE_DIR:-''${XDG_CACHE_HOME:-$HOME/.cache}/sccache}"
    export SCCACHE_CACHE_SIZE="''${SCCACHE_CACHE_SIZE:-50G}"
    unset PYTHONPATH
  '';
}
