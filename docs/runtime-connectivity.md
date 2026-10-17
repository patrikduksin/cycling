# Runtime connectivity

Firmware starts Wi-Fi without credentials and base BLE in echo mode. Saved Wi-Fi
configuration is restored after restart; SDK builds also restore the saved HRS/CSC
peer. Base keeps that saved selection but does not interpret sensor profiles.
Firmware images contain no credentials or target identifiers.

## Commands

The commands work with the debug harness enabled or disabled; `HELP` lists them.

- Requests are at most 256 ASCII bytes, including `CMD` and the request ID.
- SSIDs, passwords and BLE names are UTF-8 bytes encoded as hex, so spaces and
  line breaks cannot break framing. Use `-` for an absent BLE name or address.
- Limits: 32 bytes for SSIDs and names, 8..63 bytes for passwords. WPA2 and WPA3
  personal authentication are supported.
- BLE addresses use the little-endian byte order returned by `BLE PEERS`.

```text
CMD 1 WIFI SCAN
CMD 2 WIFI
CMD 3 WIFI NETWORKS
CMD 4 WIFI CONFIG WPA2 54657374 70617373776f7264
CMD 5 WIFI
CMD 6 WIFI CONNECT
CMD 7 WIFI
CMD 8 WIFI DISCONNECT
CMD 9 WIFI FORGET
CMD 10 BLE SCAN
CMD 11 BLE
CMD 12 BLE PEERS
CMD 13 BLE SELECT HRS 4379636c696e672053696d -
CMD 14 BLE CONNECT
CMD 15 BLE
CMD 16 BLE DISCONNECT
CMD 17 BLE SELECT CSC 4379636c696e672053696d -
CMD 18 BLE CONNECT
CMD 19 BLE FORGET
CMD 20 BLE ECHO
```

The Wi-Fi example encodes a test network `Test` with a test password; use
`mise run wifi-setup` for real credentials. `BLE SELECT` requires an SDK build;
base supports discovery, echo, disconnect and forgetting a saved selection.

## Completion and reconnection

- Mutations return `ACCEPTED`; status then reports `operation=pending`,
  `completed` or `error` with an incrementing `sequence` and a sanitized error
  code. Poll the accepted sequence before the next mutation.
- Selection and configuration save immediately and disconnect the old link. Issue
  `CONNECT` after that operation completes. `CONNECT` also reconnects an existing
  selection; `RECONNECT` is the same bounded operation.
- Wi-Fi connect completion means radio association; `state=4` means DHCP and the
  public HTTP probe also succeeded. BLE connect completion means the selected
  notification characteristic is subscribed. Connection and notification counters
  show later activity.
- Unexpected link loss keeps connection intent and retries with capped backoff.
  The failed request stays inspectable even if a background retry succeeds.
  Explicit disconnect, selection or configuration change, and forget end that intent.

## Privacy and storage

- Status, help and errors omit passwords, SSIDs and peer identifiers. `NETWORKS`
  and `PEERS` replies contain private names and identifiers; keep them in ignored
  local evidence. Password hex is still a password.
- Settings schema v7 migrates v1 through v6 records without resetting preferences.
  v6 added a bounded application area that the shell carries through every save
  without interpreting; v7 only enlarges it from 32 to 96 bytes. VANA stores its
  training profile and saved ANT sensors there. Firmware older than v7 reports a
  v7 record as unsupported and keeps it intact.
  Connectivity writes preserve saved brightness, idle and timezone values, even
  while a harness has changed them temporarily. Unsupported or malformed occupied
  settings still refuse replacement.
- Configuration uses the existing owned settings journal; ride/capture regions and
  vendor storage are untouched. Forget removes the selection from the current
  record, not every historical journal byte.

## Harness validation

`mise run wifi-setup` saves the current authorized NetworkManager profile to
`.local/wifi/config.json`. The [connectivity generator](../tools/devtools/src/cycling_devtools/harness/connectivity.py)
writes Wi-Fi and BLE scenarios to `.local/foundation/scenarios/` from the captured
original idle base state in `.local/foundation/original/state.json`, without device access.

```sh
mise exec -- uv run --project tools/devtools --locked python -m cycling_devtools.harness.connectivity
mise run harness -- run --scenario .local/foundation/scenarios/connectivity-wifi.json
mise run ble-simulator -- --profile both    # Owned host fixture for the BLE SDK scenario
mise run harness -- run --scenario .local/foundation/scenarios/connectivity-ble.json
```

Device access and port selection follow the [C606 skill](../.agents/skills/c606/SKILL.md).
Stop the BLE fixture afterward and restore the harness-enabled base.

| Scenario | Covers |
| --- | --- |
| Wi-Fi | Discovery, private provisioning, failed authentication, recovery, restart and forget |
| BLE | HRS/CSC selection, change, reconnect, notification progress, restart and forget over the real radio |

Typed steps look like `{"op":"wifi","action":"configure","profile":".local/wifi/config.json"}`
or `{"op":"ble","action":"select","mode":"sim-heart"}`. Other actions are `scan`,
`connect`, `disconnect`, `forget` and BLE `echo`. Each step waits for its own
operation sequence to complete or fail. `"completion":"error"` expects a
deliberate failure; Wi-Fi `"invalid_password":true` tests recovery without a
second private file.

Every scenario with these steps must supply `connectivity_restore` with the
actual original values; cleanup restores them through ordinary commands and
checks completion. The device does not export stored credentials.

```json
{
  "connectivity_restore": {
    "wifi": {"profile": ".local/wifi/config.json", "connected": true},
    "ble": {"mode": "authorized-heart", "connected": false, "echo_after": true}
  },
  "steps": [{"op": "wifi", "action": "scan"}]
}
```

- A null Wi-Fi profile means originally unconfigured.
- BLE modes are `none`, `echo`, `sim-heart`, `sim-csc` or `authorized-heart`; the
  last reads an explicit private authorization record.
- `echo_after` returns runtime BLE to echo while keeping the saved SDK selection.
- The generator only handles its recorded original case: base echo plus the
  authorized compiled SDK selection, which it migrates to owned settings before
  restoring echo. Restoring another owner's configuration needs a deliberate plan.

The virtual backend covers parsing, settings migration and persistence, selection
changes and notification-generation rejection; radio connect and scan are
unsupported there. It preserves a pre-existing data file and exercises torn
configuration updates.

## What this does not prove

- Scenario generation and host tests do not establish device observations; the
  recipes need the real hardware.
- The virtual backend does not emulate RF behavior or prove a real notification
  subscription.
