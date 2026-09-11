#!/usr/bin/env python3
"""Generate the ignored, device-specific BLE sensor selection."""

import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUTHORIZED = ROOT / ".local/overnight/authorized-heart-sensor.json"
OUTPUT = ROOT / ".local/ble/config.rs"


def address_bytes(value):
    parts = value.split(":")
    if len(parts) != 6:
        raise ValueError("BLE address must contain six octets")
    octets = [int(part, 16) for part in parts]
    if any(len(part) != 2 for part in parts):
        raise ValueError("BLE address octets must use two hex digits")
    return list(reversed(octets))


def configuration(mode, authorized=None):
    if mode == "echo":
        return 0, b"", None
    if mode == "sim-heart":
        return 1, b"Cycling Sim", None
    if mode == "sim-csc":
        return 2, b"Cycling Sim", None
    if mode != "authorized-heart":
        raise ValueError("CYCLING_BLE_MODE must be echo, authorized-heart, sim-heart, or sim-csc")
    if not isinstance(authorized, dict) or authorized.get("authorized_by_user") is not True:
        raise ValueError("authorized heart sensor record is missing explicit authorization")
    if "heart" not in str(authorized.get("profile", "")).lower():
        raise ValueError("authorized sensor is not an HRS peer")
    name = str(authorized.get("name", "")).encode("utf-8")
    if not name or len(name) > 29:
        raise ValueError("authorized BLE name must be 1..29 UTF-8 bytes")
    return 1, name, address_bytes(str(authorized.get("address", "")))


def render(profile, name, address):
    addr = "None" if address is None else f"Some({address!r})"
    return (
        "// Generated from ignored local authorization; do not commit.\n"
        f"pub const PROFILE: u8 = {profile};\n"
        f"pub const TARGET_NAME: &[u8] = &{list(name)!r};\n"
        f"pub const TARGET_ADDRESS: Option<[u8; 6]> = {addr};\n"
    )


def generate():
    requested = os.environ.get("CYCLING_BLE_MODE")
    if requested is None:
        requested = "authorized-heart" if AUTHORIZED.exists() else "echo"
    authorized = json.loads(AUTHORIZED.read_text()) if requested == "authorized-heart" else None
    profile, name, address = configuration(requested, authorized)
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(render(profile, name, address))
    print(f"Generated BLE mode profile={profile}")


if __name__ == "__main__":
    generate()
