# Wi-Fi bring-up

On 2026-09-11, the C606 custom firmware joined the computer's existing WPA2 network
using the ESP32-S3 integrated 2.4 GHz radio. USB logs confirmed association, a DHCP
lease, successful DNS resolution, and a TCP/HTTP request to `example.com`. The
firmware checked HTTP 200 and the page's expected heading. It then disconnected
its own station, reconnected after five seconds, obtained network configuration,
and verified the page again. Free heap was 116,240 bytes at both successful checks.

The screen displays Wi-Fi progress below the battery and button information.
Rendering and transfer continued at about 19 ms of work per frame. Companion
battery reports continued with zero bad CRCs after recovery from one or two UART
errors during radio startup. Touch probing succeeded. The earlier touch and
button interactions were physically confirmed by the user; they were not
visually rechecked during this Wi-Fi test.

The initial LAN HTTP probe timed out behind the computer's incoming firewall.
The final test uses a public website and requires no server or firewall changes
on the computer. It stops probing after success following the deliberate
reconnect, then waits for another disconnect. Failed checks retry after 30 seconds.

The implementation uses the published `esp-radio` 1.0.0-beta.0, `esp-rtos` 0.3.0,
and compatible `esp-hal` 1.1.2. Embassy provides DHCP, DNS and TCP; a 160 KiB
internal heap supports the radio and scheduler. PSRAM is not used. Espressif's
radio stack includes precompiled vendor libraries; the integrated radio is not
a fully open-spec hardware driver implementation.

The final application image is 535,840 bytes. Only the application in slot B and its OTA selection record were written through
`mise run flash`. The device workflow verified stock slot A, the bootloader and
partition table against the private baseline. No eFuses were changed.

Credentials, generated Rust configuration, configured firmware artifacts and
USB captures remain in ignored `.local/`. See [development instructions](../../../docs/development.md#wi-fi).
WPA3, TLS, BLE, power consumption and prolonged loss of the access point remain
untested. HTTP here verifies connectivity, not server identity.
