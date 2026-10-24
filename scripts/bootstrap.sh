#!/usr/bin/env bash
# Make this machine ready to develop cycling: Nix with flakes, direnv with
# nix-direnv, every checkout and agent worktree trusted, the dev shell built.
# Idempotent; rerun it after moving machines or when something looks off.
#
#   scripts/bootstrap.sh [--device-host <ssh-alias>] [--warm]
#
# --device-host  this machine has no C606; device tasks run on <ssh-alias>
#                (the machine with the USB cable, see docs/dev-setup.md)
# --warm         run `mise run verify --fast` once to fill sccache
set -euo pipefail
cd "$(dirname "$0")/.."
# Trust and name things after the primary checkout, even when run from a worktree.
root="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")"
device_host="" warm=0
while (($#)); do
  case "$1" in
    --device-host) device_host="${2:?--device-host needs an ssh alias}"; shift 2 ;;
    --warm) warm=1; shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done
config="${XDG_CONFIG_HOME:-$HOME/.config}"
step() { printf '\n== %s\n' "$*"; }

step "Nix with flakes"
if ! command -v nix >/dev/null; then
  if command -v pacman >/dev/null; then
    sudo pacman -S --noconfirm --needed nix
    sudo systemctl enable --now nix-daemon.socket
  else
    curl -fsSL https://nixos.org/nix/install | sh -s -- --daemon --yes
    # shellcheck disable=SC1091
    . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
  fi
fi
if ! nix config show experimental-features 2>/dev/null | grep -q flakes; then
  mkdir -p "$config/nix"
  echo 'experimental-features = nix-command flakes' >>"$config/nix/nix.conf"
fi
nix --version

step "direnv + nix-direnv"
command -v direnv >/dev/null || nix profile add nixpkgs#direnv
# NixOS's programs.direnv already loads nix-direnv; elsewhere load it ourselves.
if ! grep -qs nix-direnv /etc/direnv/direnvrc "$config/direnv/direnvrc"; then
  nix profile add nixpkgs#nix-direnv
  mkdir -p "$config/direnv"
  echo 'source "$HOME/.nix-profile/share/nix-direnv/direnvrc"' >>"$config/direnv/direnvrc"
fi
for shell in bash zsh; do
  rc="$HOME/.${shell}rc"
  if [[ -f "$rc" ]] && ! grep -q 'direnv hook' "$rc"; then echo "eval \"\$(direnv hook $shell)\"" >>"$rc"; fi
done
# Trust this checkout and every agent worktree of it without per-path `direnv allow`.
toml="$config/direnv/direnv.toml"
worktrees="$HOME/.t3/worktrees/$(basename "$root")"
mkdir -p "$config/direnv"
if ! grep -qsF "\"$root\"" "$toml"; then
  if grep -qs '^\[whitelist\]' "$toml"; then
    echo "Add \"$root\" and \"$worktrees\" to [whitelist] prefix in $toml by hand." >&2
  else
    printf '[whitelist]\nprefix = ["%s", "%s"]\n' "$root" "$worktrees" >>"$toml"
  fi
fi

if [[ -n "$device_host" ]]; then
  step "Device tasks go to $device_host"
  mkdir -p "$config/cycling"
  echo "export CYCLING_DEVICE_HOST=$device_host" >"$config/cycling/envrc"
  ssh -o BatchMode=yes -o ConnectTimeout=10 "$device_host" true && echo "reachable"
fi

step "Dev shell"
direnv allow "$PWD"
nix develop --command bash -c 'mise trust --quiet && rustc --version && "$CYCLING_ESP_TOOLCHAIN/bin/rustc" --version'

if ((warm)); then
  step "Warm caches"
  nix develop --command mise run verify --fast
fi
echo
echo "Ready. Open a new shell (or run 'direnv reload') in $PWD."
