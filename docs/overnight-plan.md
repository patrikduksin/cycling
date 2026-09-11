# Autonomous development plan

Approved by the user on 2026-09-11. Continue through this queue without routine
permission requests. This file is the durable handoff across compactions.

## Work order

1. #1 PSRAM, then #2 safe persistent storage.
2. #3 UI components/navigation, #4 input, #5 home/settings/device screens,
   #6 diagnostics.
3. #7 persistent settings, #8 dim/wake, #9 Wi-Fi recovery, #10 network time.
4. #11 simulated ride screen, #12 regression/stability tests, #13 performance.
5. #14 GPS, #15 Bluetooth, #16 crash diagnostics.
6. #17 ride recording/recovery, #18 history, #19 USB export/GPX.

Issues live at https://github.com/patrikduksin/cycling/issues.
The GPS issue contains the user-authorized public reference coordinates.

## Execution agreement

- GPT-5.6 Sol at medium implements each issue on a branch and runs checks and
  relevant device tests. GPT-6 Astra at medium reviews code and evidence.
- Limit review to four rounds per issue. Favor useful tested increments and
  merging; minor gaps, polish and optional improvements become TODOs or follow-ups.
- The coordinating agent checks results, publishes PR evidence, merges to main
  and closes completed issues. Partial work may merge, but unmet requirements
  remain tracked. Move to independent work when hardware blocks progress.
- Coordinate device access sequentially. Preserve stock ota_0, bootloader,
  partition table and eFuses. Use mise backup/flash/stock tasks.
- Run mise test/check/build for firmware changes. Follow AGENTS.md and
  docs/device-debugging.md. Report actual hardware observations, distinguishing
  simulated inputs and driver candidates from verified behavior.
- Sanitized images, videos and results may be attached to PRs/issues, explicitly
  authorized by the user. Never commit captures. Credentials, vendor firmware,
  dumps, identifiers and raw logs remain in ignored .local/.
- Passwordless sudo is enabled through Omarchy until 2026-09-11 13:37 -03.
  Do not assume authentication survives beyond that window.
- Finish with the latest working firmware and debugging enabled on the device.
  Report merged work, actual hardware results and remaining TODOs.

## Progress

- Planning complete. Issues #1 through #19 created.
- #1 complete: PR #20 merged as 134017e. Two reset boots detected/tested 2 MiB
  quad PSRAM; smoke and 60-second soak passed. Dedicated external allocator keeps
  normal allocations internal. Astra approved after mode/evidence corrections.
  Earlier transient heap and USB reply issues are tracked under #12.
- #2 complete: PR #22 merged as c7ff2e4. Two-sector settings journal at
  0xe98000..0xe9a000, protected by a reduced flasher image limit. Host torn-write
  tests passed; marker persisted through a second safe application reflash.
  Astra approved. Bulk SD/MMC probing remains #21, with recovered pin map in
  packages/stock/magene-c606/storage.md. Resolve bulk storage before real ride
  recording if the small settings journal is insufficient.
- #3 complete: PR #23 merged as a6f0048. Portable App/menu, shared drawing,
  release/cancel and injection ownership, session navigation restore, exact preview.
  Navigation, smoke and lease tests passed; both builds passed. Astra approved.
  Screenshots and video are linked on #23 via release evidence-2026-09-11 assets,
  never Git files. Device runs this harness-enabled build.
- #4 complete: PR #24 merged as 189a1bb. Tap bounds/slop, repeated-button held
  pointer suppression, release recovery and unknown-code policy verified by a
  44-command injected device scenario. Both builds passed; Astra approved.
- #5 complete: PR #25 merged as d3b0dfa. Home/settings/device screens, brightness
  slider, status placeholders and host DEBUG stream resynchronization. Both builds,
  30 Rust/19 Python tests, 28-frame device scenario and visual inspection passed.
  Astra approved; screenshots/transition video linked in PR. Coordinator handled
  final commit/publication after implementation and review were complete.
- #6 complete: PR #26 merged as 0d7f2f9. Diagnostics screen, live frame metrics
  and one-second display snapshot. Device navigation/value comparison passed;
  stable heap and no new CRC/UART/touch errors. Astra approved. Screenshot linked
  in PR predates tiny X glyph fix; next safe flash must deploy that fix.
- #7 complete: PR #27 merged as 7207e2b. Versioned preferences, idle debounce,
  temporary-session isolation, explicit PERSIST and automated restart test.
  Seven device reports prove temporary65 does not survive reflash, explicit65
  does, and original50 is restored. Writes34–35ms, no new CRC/UART or retained
  heap loss. Both builds/checks passed; Astra approved. Device includes X glyph.
- #8 complete: PR #28 merged as f9c7fb1. Persistent timeout/OFF and dim level,
  shared wake consumption, selected/effective brightness and temporary idle controls.
  Device test verified dim despite heartbeats, held-touch wake, button/tap wake
  and END restoration. PWM commands observed; actual luminance is not measured.
  Astra approved with a final portable gesture fix and regression test; checks
  passed. Screenshot linked in PR. Next flash deploys that narrow gesture fix.
- #9 complete: PR #29 merged as 88f7897. Bounded connection/DHCP/request waits,
  capped retries, generation-scoped recovery, truthful states and controlled faults.
  Three requested reconnects and two same-link probe fault recoveries passed.
  Heap116288 to116240, no new CRC/UART errors. Actual AP loss and DHCP timeout
  were not forced on hardware. Astra approved; required checks/both builds passed.
  Device runs enabled build, including final #8 gesture fix.
- Starting #10 network time. Preflights for UI, networking, settings, dimming,
  Bluetooth and GPS are saved in ignored .local/overnight/. GPS trace
  now supports UART0 RX0/TX1, startup 921600 baud, companion power callbacks.
  These remain stock-code findings pending actual device validation.
- For later media, use unique sanitized filenames under .local/overnight/evidence,
  inspect them, upload with gh release upload evidence-2026-09-11, then link from PR.
  Browser attachment UI is unavailable. Never upload raw logs or credentials.

Update this section after each issue with branch/PR, verification, review outcome,
and the next task. Keep raw evidence in .local/ and link written results in PRs.
