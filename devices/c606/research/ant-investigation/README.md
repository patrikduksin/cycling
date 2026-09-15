# ANT companion investigation

Research date: 2026-09-15. Application checkout: `82e7241`.

## Findings for the next decision

| Question | Result | What this enables next |
| --- | --- | --- |
| [1. Identity](01-identity.md) | The connected companion reports version **1.902** using stock's version formatting. FCC internal photos identify nRF52810-QFAA on the certification board; the installed binary remains unverified. | Use N22 1.902 as the best matching research image. |
| [2. Capacity](02-capacity.md) | Both examined N22 images request **12 ANT channels**. The newer image assigns ten fixed sensor channels and two discovery channels. | Test five or more different sensor types; do not advertise ten verified connections yet. |
| [3. Same-type sensors](03-identity-routing.md) | The companion chooses channels by type and removes sender identity from forwarded data. | Keep one active peer per type. Saved alternatives can still be implemented above the bridge. |
| [4. Scanning](04-scanning.md) | The recovered scan path uses channels 0 and 10, without closing the ten sensor channels. | Concurrent discovery is a strong candidate for a controlled hardware experiment. |
| [5. Outgoing commands](05-transmission.md) | The outgoing call matches ANT acknowledged transmission. Its result is discarded, and the examined bridge event handler omits TX completion/failure events. | Test a bounded, profile-specific request with an observable sensor response. |

These findings narrow the investigation; they do not establish ten-sensor operation,
uninterrupted discovery or successful over-air command delivery.

## Work performed

- Re-decoded the original N22 1.628, N22 1.902 and N21 1.956 packages. Their outer
  CRCs passed and decoded payloads matched the retained extracted images.
- Traced ARM Thumb control flow, fixed channel mapping, initialization, scan,
  identity serialization and transmit/event handling. Checked stock ESP32 version
  formatting and scan-request construction.
- Compared the recovered radio calls with vendor-authored ANT API headers and
  Nordic's published API descriptions. The exact installed SoftDevice release
  remains unknown; an ABI match is not a stack-version identification.
- Used the supported no-reset terminal for `INFO`, one `COMPANION QUERY`, then
  `COMPANION`. The device reported base/harness source `6028bf0ad9fb`, no recording,
  and a fresh identity reply. No flash, reset, scan, sensor connection, outgoing
  ANT message or persistent setting change was performed.

Follow-up research explains the separate discovery networks, verifies TX wrapper
behavior through offline instruction execution, and traces stock power-calibration
sending. See the [evidence appendix](06-follow-up-evidence.md). The user-supplied
FCC internal-photo PDF independently identifies nRF52810-QFAA
on the certification board, with PCB revision V5.0.2 visible. The UART interface
cannot expose chip identity or installed stack bytes.

Raw firmware, disassembly, downloaded headers, verification metadata and USB
records remain ignored in `.local/ant-research/`. No firmware implementation changed.
Reports contain findings, not vendor code. Existing requirements remain in
[#90](https://github.com/patrikduksin/cycling/issues/90).

## Artifact provenance

SHA-256 values refer to decoded application payloads, not installed readbacks.

| Image | SHA-256 |
| --- | --- |
| N22 1.902 | `e7e46db0b2f0f5be7a9fe4b10ef9996e15870f148fb4923df4833b9ee4dde6e5` |
| N22 1.628 | `3b92119bece04a025b8fd2eee7555328d722f8eb20760a57d5e9925fab19ab10` |
| N21 1.956 | `9ce38974fb3457b05dae3e3bdc1392a2a74cecbe5816c779b929c385455ce5e4` |

N22 addresses use flash base `0x12000`. Old notes mentioning three channels are
historical; [PR #109](https://github.com/patrikduksin/cycling/pull/109) records the
four-sensor workout evidence. Nothing here supersedes its measurement limitations.
