"""Save a private Wi-Fi profile for ordinary runtime USB provisioning."""
import argparse
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
LOCAL = ROOT / ".local/wifi"
CONFIG = LOCAL / "config.json"


def save(path, text):
    LOCAL.mkdir(parents=True, exist_ok=True, mode=0o700)
    LOCAL.chmod(0o700)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    os.fchmod(fd, 0o600)
    with os.fdopen(fd, "w") as stream:
        stream.write(text)


def nmcli(*args):
    result = subprocess.run(["nmcli", "--escape", "no", *args], capture_output=True, text=True)
    if result.returncode:
        raise ValueError("NetworkManager query failed; no credentials printed")
    return result.stdout.rstrip("\n")


def setup():
    devices = [line.split(":")[0] for line in
               nmcli("-t", "-f", "DEVICE,TYPE,STATE", "device", "status").splitlines()
               if ":wifi:connected" in line]
    if len(devices) != 1:
        raise ValueError("Expected one connected Wi-Fi device")
    device = devices[0]
    uuid = nmcli("-g", "GENERAL.CON-UUID", "device", "show", device)
    ssid = nmcli("-g", "802-11-wireless.ssid", "connection", "show", uuid)
    security = nmcli("-g", "802-11-wireless-security.key-mgmt", "connection", "show", uuid)
    if security not in ("wpa-psk", "sae"):
        raise ValueError("This setup helper supports WPA2/WPA3 personal networks")
    password = nmcli("--show-secrets", "-g", "802-11-wireless-security.psk", "connection", "show", uuid)
    config = dict(ssid=ssid, password=password, security=security)
    validate(config)
    save(CONFIG, json.dumps(config, indent=2) + "\n")
    print("Saved current Wi-Fi profile in ignored .local/wifi/.")


def validate(config):
    if not isinstance(config.get("ssid"), str) or not 1 <= len(config["ssid"].encode()) <= 32:
        raise ValueError("SSID must contain 1 to 32 UTF-8 bytes")
    if "\x00" in config["ssid"]:
        raise ValueError("SSID cannot contain NUL")
    password = config.get("password")
    if not isinstance(password, str) or not 8 <= len(password.encode()) <= 63 or password == "<hidden>" or "\x00" in password:
        raise ValueError("An accessible 8 to 63 byte Wi-Fi password is required")
    if config.get("security", "wpa-psk") not in ("wpa-psk", "sae"):
        raise ValueError("Unsupported Wi-Fi authentication")


def command(config):
    validate(config)
    auth = 'WPA3' if config.get('security') == 'sae' else 'WPA2'
    return f'WIFI CONFIG {auth} {config["ssid"].encode().hex()} {config["password"].encode().hex()}'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("setup",))
    action = parser.parse_args().action
    try:
        {"setup": setup}[action]()
    except (ValueError, OSError, KeyError, IndexError) as error:
        # Do not print exception contents: parsers may include secret input.
        raise SystemExit(f"Wi-Fi {action} failed ({type(error).__name__}); check private config and NetworkManager access.") from None


if __name__ == "__main__":
    main()
