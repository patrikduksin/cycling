#!/usr/bin/env python3
"""Validate private runtime BLE selections for the USB harness."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUTHORIZED = ROOT / ".local/overnight/authorized-heart-sensor.json"


def address_bytes(value):
    parts = value.split(":")
    if len(parts) != 6:
        raise ValueError("BLE address must contain six octets")
    try:
        octets = [int(part, 16) for part in parts]
    except ValueError:
        raise ValueError("BLE address must contain hex octets") from None
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
        raise ValueError("BLE mode must be echo, authorized-heart, sim-heart, or sim-csc")
    if not isinstance(authorized, dict) or authorized.get("authorized_by_user") is not True:
        raise ValueError("authorized heart sensor record is missing explicit authorization")
    if "heart" not in str(authorized.get("profile", "")).lower():
        raise ValueError("authorized sensor is not an HRS peer")
    name = str(authorized.get("name", "")).encode("utf-8")
    if not name or len(name) > 29:
        raise ValueError("authorized BLE name must be 1..29 UTF-8 bytes")
    return 1, name, address_bytes(str(authorized.get("address", "")))


def command(mode, authorized=None):
    profile, name, address = configuration(mode, authorized)
    if profile == 0:
        return 'BLE FORGET'
    peer = '-' if address is None else bytes(address).hex()
    return f'BLE SELECT {"HRS" if profile == 1 else "CSC"} {name.hex() or "-"} {peer}'
