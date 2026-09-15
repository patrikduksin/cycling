# Runtime connectivity

Firmware starts Wi-Fi without credentials and base BLE in echo mode. Saved Wi-Fi
configuration restores after restart. SDK builds also restore the saved HRS/CSC
peer. Base preserves that saved selection but does not interpret sensor profiles.
No credentials or target identifiers are compiled into firmware images.

The ordinary USB protocol works with the debug harness enabled or disabled.
`HELP` lists the available commands. Requests are bounded to 256 ASCII bytes,
including `CMD` and the request ID. SSIDs, passwords and BLE names use UTF-8 bytes
encoded as hex so spaces and line breaks cannot change command framing. Use `-`
for an absent BLE name or address. Addresses use the little-endian byte order
returned by `BLE PEERS`. Inputs are limited to 32 SSID/name bytes and 8..63 password
bytes. WPA2 and WPA3 personal authentication are supported.

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

The Wi-Fi example encodes a test network named `Test` with a test password.
Use the private profile helper for real credentials. `BLE SELECT` requires an SDK
build. Base supports discovery, echo, disconnect and forgetting a saved selection.
`CONNECT` also reconnects an existing selection; `RECONNECT` uses the same bounded
operation. Selection/configuration saves immediately and disconnects the old link.
Issue `CONNECT` after the configuration operation completes.

Mutations return `ACCEPTED`, then status reports `operation=pending`, `completed`
or `error`, with an incrementing `sequence` and a sanitized error code. Poll the
accepted sequence before another mutation. Wi-Fi connect completion means radio
association. `state=4` means DHCP and the public HTTP probe also succeeded. BLE
connect completion means the selected notification characteristic is subscribed.
Connection and notification counters expose subsequent activity. Unexpected link
loss retains connection intent and retries with capped backoff. The failed request
remains inspectable even if a background retry succeeds. Explicit disconnect,
selection/configuration change and forget stop that reconnect intent.

Normal status/help/errors omit passwords, SSIDs and peer identifiers. Explicit
`NETWORKS` and `PEERS` discovery replies contain private names/identifiers; keep
those replies in ignored local evidence. Password hex is still a password.

Settings schema v5 migrates v1 through v4 records without resetting their preferences.
Connectivity writes preserve previously saved brightness, idle and timezone values,
including when a harness temporarily changes those values. Unsupported/malformed
occupied settings still refuse replacement. Configuration uses the existing owned
journal; ride/capture regions and vendor storage are untouched. Forgetting removes
the selection from the current record, not every historical journal byte.

## Harness validation

`mise run wifi-setup` saves the current authorized NetworkManager profile privately.
`tools/devtools/src/cycling_devtools/harness/connectivity.py` generates Wi-Fi and BLE scenarios from the
captured original idle base state. It does not access the device.

```sh
export CYCLING_PORT="$(mise run device-port)"
mise exec -- python -m cycling_devtools.harness.connectivity
mise run harness -- run .local/foundation/scenarios/connectivity-wifi.json
# Start the existing owned host fixture before the BLE SDK scenario:
mise run ble-simulator -- --profile both
mise run harness -- run .local/foundation/scenarios/connectivity-ble.json
```

The port helper reads sysfs and requires exactly one VID/PID and identity match
to the private backup manifest; it does not open USB. Stop if discovery fails.
Repeat `export CYCLING_PORT="$(mise run device-port)"` after flashing,
reconnection or any port renumbering before running the next terminal or harness.
`mise run device-port` also prints the verified path.

Run the BLE fixture as an owned process, stop it after the test, and restore the
harness-enabled base with the protected firmware workflow. The Wi-Fi scenario tests
discovery, private provisioning, failed authentication, recovery, restart and forget.
The BLE scenario tests HRS/CSC selection, change, reconnect, notification progress,
restart and forget through the real radio. These recipes require appropriate
hardware; generation or host tests alone do not establish device observations.

A typed step is `{"op":"wifi","action":"configure","profile":".local/wifi/config.json"}`
or `{"op":"ble","action":"select","mode":"sim-heart"}`. Other actions are
`scan`, `connect`, `disconnect`, `forget`, plus BLE `echo`. A deliberately failed
connection uses `"completion":"error"`. Wi-Fi fixture configuration can use
`"invalid_password":true` to test recovery without writing a second private file.
Each scenario must supply original restoration values. This example preserves the
previously compiled authorized SDK peer in owned settings, then returns runtime
BLE to echo without clearing that saved selection:

```json
{
  "connectivity_restore": {
    "wifi": {"profile": ".local/wifi/config.json", "connected": true},
    "ble": {"mode": "authorized-heart", "connected": false, "echo_after": true}
  },
  "steps": [{"op": "wifi", "action": "scan"}]
}
```

A null Wi-Fi profile means originally unconfigured. BLE modes are `none`, `echo`,
`sim-heart`, `sim-csc` or `authorized-heart`; the latter reads an explicit private
authorization record. `echo_after` restores runtime echo while retaining that
saved SDK selection. Supply the actual original values. The device does not export
stored credentials. The generator accepts only its recorded original base echo
case plus its authorized compiled SDK selection. It migrates that selection to
owned settings, then restores echo for the base composition. Supply a deliberate
restoration plan for another owner's configuration.

The virtual backend tests production parsing, settings migration/persistence,
selection changes and notification-generation rejection. Radio connection and
scan operations remain explicitly unsupported there. Virtual tests preserve a
pre-existing data file and exercise torn configuration updates; they do not emulate
RF behavior or prove a real notification subscription.
