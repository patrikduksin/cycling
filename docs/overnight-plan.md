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

## Follow-up queue authorized on 2026-09-11

The user authorized continuing through all remaining issues with the same
implementation, review, evidence and merge workflow. This supersedes the earlier
completed-queue stop note for these follow-ups. Work in this order:

1. #43 USB debug session handoff, to make unattended hardware testing reliable.
2. #34 GNSS transport loss, epoch metadata and receiver/control investigation.
3. #36 BLE MTU23 interoperability and continuous sensor acquisition.
4. #21 read-only SD/MMC hardware probing and identification.
5. #44 visible ride capacity and explicit reclaim after verified export.

Sol medium implements one issue at a time; Astra medium reviews within four
rounds. Independent read-only preflights may run alongside implementation. Keep
hardware and source ownership explicit. Establish a failing reproduction for
bugs, test candidate fixes, and favor useful tested merges. Hardware-dependent
requirements may remain open with exact evidence; do not close an unverified
requirement just because a partial increment merged.

Existing four rides are agent-created test records and have two identical
verified raw exports plus the final post-soak export under .local/exports/.
Before any reclaim test, export current contents again and verify identity;
exercise destructive paths only on identified disposable test data, confined
to the owned ride reservation. Never write the vendor bulk-storage filesystem.

At follow-up start12:28UTC, sudo works and its original expiry is still
13:37Chile/16:37UTC today. Do not assume the window renews automatically.
Starting baseline main e7df373, enabled device Home, four rides/slot22.

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
- #10 complete: PR #30 merged as 67ffd3e. Validated bounded SNTP, UTC anchored
  to monotonic time, offline/stale status and fixed timezone preferences.
  Device Cloudflare sync, offline progression and reconnect resync passed;
  screenshots linked in PR. Host comparison is within a194ms command window,
  not a millisecond accuracy guarantee. No RTC/DST; NTP era0 limitation documented.
  Astra approved;43 Rust/19 Python/check/both builds passed.
- #11 complete: PR #31 merged as 914f532. DEMO ride with deterministic speed,
  monotonic distance/time, two pages/layouts and start/pause/resume/reset.
  Device27-frame recording verified exact values, pause freeze and cleanup;
  heap116288 unchanged, no new CRC/UART errors. Media linked in PR.
  Astra approved;47 Rust/19 Python/check/both builds passed. Also scoped the
  workflow token to espup setup after intermittent GitHub403 download failures.
- #12 complete: PR #32 merged as 7ef6185. Repeatable functional/visual/cleanup/
  persistence suite and bounded soak. Corrected soak602.013s/562samples passed;
  frame6201 to20349, companion8773 to28805, heap116240 before/after cleanup,
  sampled minimum116192, no new CRC/UART/touch errors. First soak exposed a
  host Back-navigation bug; fixed with fake-device tests and reran only soak.
  Earlier failure is documented separately. Astra approved;47 Rust/24 Python,
  check/build passed. Device remains enabled with original settings restored.
- #13 complete: PR #33 merged as be3152e. Exact image comparison skips unchanged
  LCD writes using16960 bytes external PSRAM. Typical sampled work20 to3ms;
  changed Ride frame22ms, same loop frequency/bus timing and no power claim.
  Static windows0draws/115skips, Ride4/112. Heap unchanged; both builds/hardware
  baselines passed, enabled restored, targeted regression/60s soak passed.
  Astra approved;48 Rust/24 Python/check passed.
- #14 complete: PR #35 merged as 1f8590d. Live GN NMEA verified on UART0
  RX0 at921600, bounded parser and interrupt-fed8KiB ring, GPS screen/debug.
  Selected30.055s connected coexistence window clean with639 valid sentences;
  setup/capture/navigation and Wi-Fi reassociation detected UART losses and
  recovered. Indoor nofix/sats0, no positional claim. Stock open candidate sent
  without verified effect. Follow-up #34 covers outdoor fix, model/control,
  epoch association and loss cause. Astra approved;52 Rust/24 Python/check/both
  builds passed. Screenshot linked on PR; enabled firmware remains installed.
- #15 complete: PR #37 merged as 5c3d53b. BLE scan and eight-byte GATT echo
  verified with owned laptop across two reconnect rounds at ATT MTU60, including
  exact notifications/readback and malformed-write rejection. Owned HRS/CSC
  simulator produced73BPM/60RPM and returned to peripheral advertising. Concurrent
  30s Ride/Wi-Fi/BLE window kept heap80168..80308, companion clean; GPS UART+3
  with recovery. All64 GPS samples were fresh,7.6..11.4m from the authorized
  reference, ages0..941ms; #34 updated. #36 tracks MTU23 interoperability and
  continuous/real sensor support. Astra approved after2rounds;56 Rust/24 Python,
  check/bothbuilds passed. Enabled firmware installed; laptop test peers and
  advertisements cleaned up. Written evidence linked in PR; raw data ignored.
- #16 complete: PR #38 merged as ff5137d. Seven-word RTC panic marker,
  reset classification, retained version in USB, Diagnostics status and controlled
  PANIC/RESTART. Final hardware run read software/controlled/0.1.0 after panic,
  then software/none/empty after clean restart. Preferences unchanged; display
  frame1..49 and companion0..72 in2s. Astra reviewed; visual masks corrected;
  58 Rust/24 Python/check/both builds passed. Enabled firmware installed.
  Clean-restart screenshot attached to PR; raw backtrace ignored. Retention proof
  covers software reset only, not power loss, watchdogs or cache-off failures.
- #17 complete: PR #39 merged as 1b66aec. Owned1MiB ride region
  D98000..E98000, app ceiling638000, settings unchanged; explicit verified init,
  256-byte committed batches, live/demo provenance and recovery at last commit.
  Init256sectors max41ms withGPS UART+19; normal appends0..1ms with no new
  GPS/companion errors in tested sessions, heap~80KiB. Final scenario keptslot11
  during temporary input, recovered demo atslot17, then saved5live samples to
  slot22 with4saved/recovered rides. Live GPS was nofix; no persisted-position
  claim. Up to4in-flight samples may be lost; uncertain writes require restart.
  Astra reviewed3rounds;64 Rust/25 Python/check/bothbuilds passed. Live screenshot
  attached. Next flash must deploy final same-frame recorder display refresh.
  Physical controls currently start demo; live start is USB-only, documented TODO.
- #18 complete: PR #41 merged as b326790. Latest four completed summaries in a
  fixed array, two cards/page, total count retained; complete records remain for
  export. Four rides/slot22 survived flash and both captured pages. Final targeted
  window GPS UART0 to0, companion UART1 to1, heap80276 stable, max frame25 to32ms.
  Earlier setup/capture window GPS+2/max912ms remains unresolved and documented.
  Astra approved after2rounds;66 Rust/25 Python/check/bothbuilds passed. Both
  screenshots attached outside Git. Enabled firmware installed, including #17's
  final same-frame recorder refresh.
- #19 complete: PR #42 merged as 5af6b36. Read-only bounded full-prefix USB
  export, raw preservation, independent format/CRC validation, JSON and GPX1.1.
  Intentional interruption after256bytes then two identical5632byte/22slot retries.
  Root independently checked every commit/CRC and all9fields of24samples;
  four rides had10/4/5/5samples, no invalid slots. No recorded positions in these
  rides, so explicit no-route result; GPX verified with synthetic fixtures.
  Slot22/rides4/prefs50/30/10/0/heap80308/GPS UART0/companion UART1 unchanged.
  Astra approved2rounds;66 Rust/32 Python/check/bothbuilds passed. Enabled firmware
  installed; private exports ignored, sanitized written proof attached to PR.
- #40 complete: PR #45 merged as 581f2bc. On-device Live/Demo selection,
  preserved History navigation, shared source/elapsed presentation and restored
  temporary selection. Device Ready Live elapsed0; temporary Running2180ms;
  speed/distance unavailable. Screenshots/video attached outside Git. Final
  required checks:67 Rust/36 Python/check/bothbuilds; Astra reviewed within the
  four-round limit. README now describes current features and limits.
- Final integration: functional/visual/cleanup/persistence passed in
  .local/tests/issue40-final/. A post-flash Scanning precondition exposed a
  missing wait in the host helper; bounded20s scan wait added/tested/reviewed.
  Continued with .local/tests/issue40-final-soak/:602.034seconds,581samples,
  frames878 to15007, companion1249 to21325; heap80308 to80260, minimum80132,
  PSRAM free2080192. No new companion CRC/UART or touch errors. GPS added
  1080425bytes/12571valid sentences, no checksum/parse/ring/line-overflow errors,
  but47 UART errors. This is not loss-free GNSS evidence; follow-up #34 remains.
  Root reconciled both stages: four rides/slot22 and prefs50/30/10/0 unchanged.
- Final coordinator export .local/exports/final-preserved/ matched every5632byte
  of the #19 baseline exactly:22slots,4rides,24samples. Proof comment on PR#45.
  Final state .local/tests/final-handoff/: enabled firmware, Home, Demo selected,
  recorder ready, no active test session/recording, Wi-Fi connected/time fresh,
  heap80260 and PSRAM free2080192. GPS nofix indoors at handoff; earlier actual
  fixes near the authorized reference remain separately documented under #34.
- Overnight queue is complete: #1 through #19 plus #40 merged. Remaining tracked
  work: SD/MMC probing #21; GNSS model/control/epoch/loss #34; continuous BLE and
  MTU23 interoperability #36; occasional truncated USB startup prefix #43;
  capacity display/explicit ride reclaim #44. Current journal has roughly4.5h
  total1Hz sample capacity before event overhead, with no reclaim workflow yet.
  Do not relaunch this completed queue after compaction. Next work starts from
  those follow-ups or new user direction. Sudo expiry remains13:37 -03 today.
- For later media, use unique sanitized filenames under .local/overnight/evidence,
  inspect them, upload with gh release upload evidence-2026-09-11, then link from PR.
  Browser attachment UI is unavailable. Never upload raw logs or credentials.

Update this section after each issue with branch/PR, verification, review outcome,
and the next task. Keep raw evidence in .local/ and link written results in PRs.
