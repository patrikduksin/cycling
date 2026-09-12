# Autonomous development plan

Approved by the user on 2026-09-11. Continue through this queue without routine
permission requests. This file is the durable handoff across compactions.

## Active refactor session, 2026-09-12

This section supersedes historical queue, device and agent instructions below.
The user authorized this new session to complete epic #54 and live child issues
#55 through #66 autonomously, including implementation, independent review,
publication, merges, issue closure when fulfilled, safe flashing and rebooting.
Do not restart the completed historical queues. #34 retains its separate limits.

The user explicitly authorized custom firmware on the connected USB C606 and
leaving the verified device/core build running with ordinary terminal/logging and
the development harness enabled. Preserve stock ota_0, bootloader, partition
table, eFuses and all existing persisted data. Use repository backup/flash/stock
tasks. No vendor filesystem writes. Raw evidence stays in ignored `.local/`.
Passwordless sudo was verified at 15:12 UTC. The reported 250-minute window
started around or before 15:10 UTC; recheck availability and prioritize hardware
validation. Do not modify system security or assume renewal.

Root owns architecture, integration, publication and ALL serialized device access.
Use GPT-6 Astra low for bounded implementation, medium for persistence/concurrency
ownership and independent review. At most two implementers and one reviewer may
work alongside root, with explicit file/worktree ownership. Reviews block concrete
correctness, agreed boundary violations, protected data risks and missing checks.
After two substantive fix rounds, split scope or escalate instead of polishing.
Run mise test/check/build for firmware changes, both harness modes for feature
boundaries, and actual device checks for changed hardware access/display timing.

Current starting state: clean main at eb0c7d2. Live #54-#66 bodies fetched at
15:12 UTC to private `.local/refactor-session/issue-*.json`. Historical state says
stock selected; current boot/USB state has not yet been observed. Root owns device;
no agent or collector owns a serial session. No source edits preceded this session.

User clarification at session start: C606 is indoors. User confirms the existing
GPS implementation works well. This is user-confirmed baseline, not new measured
evidence. Validate transport/parser progression, no-fix/freshness and recovery;
lack of an indoor fix alone is not a regression or blocker. Preserve the proven
GPS behavior and do not claim new outdoor accuracy.

Progress at 15:40 UTC: #55 closed by merged PR #67, b6f680c. #56 closed by
merged PR #68, 193c133, implementation e8155ea. Both independently approved by
Astra medium. Board test/check/both builds passed. Enabled hardware window
152318..162472 ms advanced GNSS valid3333..3543 and companion5033..5369 with all
UART/DMA/CRC/touch counters zero and heap80336 unchanged. Preferences100/30/20/-180
and one saved demo/four slots remain. `.local/exports/refactor-baseline` and
`refactor-board` are byte-identical. Captured Home pixels inspected; physical
panel not observed. Disabled mode touch, battery/display/Wi-Fi/HTTP/time observed;
short disabled sample is not GNSS interval evidence. First enabled postflash USB
open was silent; documented monitor release booted it and retry passed.

Progress at 16:34 UTC: PRs #67/#68/#69 merged; #55–64 closed. Service merge
is `0539c03`. Final publication branch is `refactor/validation`; it includes
checkpoint660affd and host-only validation fixes. #54/#65/#66 await the final PR
merge/closure. All live #54–66 issue bodies were rechecked and remain unchanged.
Astra-medium final source, documentation and hardware evidence review approved
#65/#66 closure. Required checks pass: 54 base, 87 SDK, 15 pinned BLE vendor and
55 Python tests, boundary/fmt/Clippy checks and four firmware combinations.
The final build-script check also passed; no firmware logic changed after8bc6482.

The C606 is running **660affdce26e, clean base, harness enabled, SDK disabled,
INFO logging, no capture**. Final device timestamps270625–270884ms confirmed
receiving/no-fix GNSS5799valid, companion8985valid, all current tracked faults0,
Wi-Fi verified/fault0, fresh network time, echo advertising, crashnone and
preferences100/30/20/-180 without persistence error. Display black fill completed
at13ms maximum. All serial readers, tests and owned BLE fixtures have stopped;
CDC driver was restored after the reconnect test. Root has exclusive ownership;
no further device mutation is needed. Firmware ELF/image snapshots are private
under `.local/refactor-session/firmware-660affd.*`.

[Sanitized validation](refactor-validation.md) records all four configurations,
settings save/restart/restore, SDK disabled export, owned CSC reconnect/freshness,
12samples with USBclosed, 19-sample saved ride and four-sample recovered ride.
The last SDK export contains3rides/18slots and preserves the original1024-byte
prefix exactly. Two identified disposable records were appended; no ride erase,
initialization or reclaim ran. The final base flash used protected repository
logic and the base has no ride writer. Original settings remain preserved.

Final base tests observed actual GNSS DMA/UART loss and companion UART loss under
a six-second stall, then sustained recovery; injected Wi-Fi failure/recovery;
controlled panic and a clean one-shot marker reset; 25seconds with hostclosed
and visible logloss without acquisition faults; CDCport removal/reconnect with
bounded backoff and the same boot identity. Two host startup misclassifications
were reproduced from private traces and fixed with independent review; their
failed reports remain. The panic retry and final evidence pass. Raw logs, private
identifiers and exports stay ignored. Firmware image803552bytes; queue3124bytes;
final sampled freeheap80308/min79860, enqueue max1829µs and USBpump max310µs are
observations, not peak memory, WCET or power measurements.

The user-confirmed indoor GPS baseline remains separate from new no-fix/parser
and recovery evidence. #34 stays open for its unverified receiver/control/outdoor
requirements. Physical panel/switch/touch accuracy and power were not newly
established. BLE remains onepeer with upstream queue-lag limitations. Explicit
unknown-settings recovery is follow-up #70; normal SAVE remains non-destructive.
Next: publish and merge the reviewed validation/host-fix PR, close #65/#66/#54,
confirm repository/issue state and return the concise handoff. No routine user
approval or new hardware test is required.

Baseline safe `sudo -n -E mise run flash` reused existing backup with safeguards.
Ordinary user lacks USB permissions; sudo task works. Sudo builds create root-owned
.local artifacts; root restores ownership narrowly before normal builds. Signing
helper cannot reach1Password; per-command unsigned commits used without config
changes. No system security changes. Raw evidence stays private. No agent has
permission to independently touch USB/Bluetooth hardware. No persistent collector
or test peer should remain at final handoff.


## Historical latest device state

At the user's request after the GPS work, `mise run stock` completed successfully
on September 11. Stock slot A was selected and the device reset. The stock UI
was not independently observed. Leave stock selected until the user asks to
resume custom firmware. This supersedes earlier custom-firmware handoff notes.

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

## Follow-up progress

- #43 complete, PR #46, bf37a22. Host cleanup stops heartbeats before END.
  The forced handoff reproduction failed 5/12 before the fix; 20/20 forced and
  10/10 ordinary heartbeat sessions passed afterward. The parser stays strict.
- #34 partial increment merged in PR #47, a935766. UART0 uses an 8 KiB UHCI DMA
  stream with separate UART/DMA fault reporting and bounded recovery. Final
  451-request stress, Wi-Fi reconnect and capture runs advanced GPS parsing with
  no new faults. Forced starvation recovered. Satellite counts match fractional
  coordinate epochs. Stock close/open commands paused and resumed the stream,
  without proving electrical power behavior. Receiver identity is unknown and
  outdoor validation remains open; the user confirmed the device is indoors.
- #36 complete, PR #48, 3ddf13f. A licensed pinned Trouble Host patch fixes
  minimum-MTU discovery. Two MTU 23 sessions passed discovery/write/notify/read
  and rejected short writes without changing data. Continuous one-peer HRS or
  CSC has freshness/contact/reconnect handling, live UI and recording support.
  Real Polar H10 and laptop CSC-only fixture tests passed. The HRS coexistence
  run received 46 notifications with no new GPS/companion faults and stable heap.
  Real readings and peer identifiers remain private. The simulator screenshot
  was attached outside Git. Simultaneous separate sensors remain unsupported.
- #21 complete, PR #49, a2e5892. The opt-in one-bit native probe identified MMC
  product field 004GA1, capacity 3,959,422,976 bytes and 512-byte sectors. Bounded
  repeated reads matched across two reset boots. CLK13/CMD14/D016 are verified;
  D1-D3 remain candidates. No write ownership was granted. Stock image verification
  and slot selection passed, but stock UI/filesystem operation was not observed.
  Normal no-probe HRS/harness firmware was restored. Vendor build artifacts are
  ignored. All required tests and probe/default/disabled builds passed.
- #44 complete, PR #50, c03358c. Capacity is visible before riding. Explicit clear
  verifies a supplied export against the current prefix hash and checks the
  exported slot bound before erasing. Each owned sector is erased/read-verified;
  there is no automatic erase or blind mutation retry. Interrupted-clear and
  refusal paths are covered by shared fake-media/host tests. Hardware preserved
  the known five-ride export, cleared to a verified zero-byte export, then saved
  and exported a four-sample demo ride. The capacity screenshot was attached
  outside Git. Tests: 84 firmware, 15 vendor and 45 Python; check and both builds
  passed. Astra approved each merged issue within two review rounds.

Current device: normal no-probe HRS/harness firmware, Home/Ready, one saved demo
ride in slots 0..3, next slot 4. Latest observed preferences are brightness 100,
timeout 30, dim 20, timezone -180. Preserve these, not historical test settings.
The old five rides remain in private verified exports, including
.local/exports/1789136629778452425 and .local/exports/1789134457934860141.
Empty/new exports: .local/exports/issue44-empty and issue44-new-demo-final.

- #51 complete, PR #52, d71358c. A final settled check exposed companion UART
  loss and led to a controlled reproduction. Instrumented polling received 412
  valid reports with three FIFO-overflow reports in 12.068 seconds. Matched
  brightness 50/100 tests each added four errors. UART2 now feeds a 2 KiB ring
  from a bounded interrupt handler. Decoder reset precedes post-loss data, and
  optional GPS-open TX failure cannot disable companion RX. Host tests cover
  overflow/resynchronization. Final normal HRS/harness replay received 732
  reports in 20.059 seconds with no new UART/CRC faults, advancing GPS, connected
  HRS and stable 80,336-byte free heap. Brightness, heartbeat, reconnect, capture
  and quiet comparison windows also passed. Settings and the one demo ride were
  preserved. Astra approved after two review rounds plus final evidence review;
  87 firmware, 15 vendor and 45 Python tests, check and both builds passed.
  Raw evidence remains in .local/tests/companion-*. No hardware ring-exhaustion
  fault or physical button presses were forced during this follow-up.

The authorized follow-up queue is complete. Only #34 remains open for measured
outdoor reference accuracy, fitted-receiver identification and electrical power
behavior beyond proven stream control. Outdoor acquisition is now user-confirmed.
On September 11 the user reported NO FIX indoors; live parsing advanced without
faults, and a private trace showed receiver-invalid GGA/RMC reports. The user
then took the powered device outdoors on battery and reported successful GPS.
After reconnecting indoors, telemetry showed a fix accepted 140,322 ms earlier.
No raw outdoor position was captured, so no reference-accuracy claim is made.

PR #53, 0454edd, adds WAITING FOR FIX, a fresh satellites-used count and units for
last-fix age. It does not change position validity or receiver configuration.
Astra approved the trimmed change; 87 firmware, 15 vendor and 45 Python tests,
check and both builds passed. Final normal HRS/harness firmware is installed,
with one ride/slot 4 and preferences 100/30/20/-180 preserved. The inspected GPS
screenshot was attached outside Git; raw evidence is in
.local/tests/gps-diagnostics-screen and gps-outdoor-return-direct. Receiver
identity and accuracy follow-up remains #34. No further GSA/GSV draft is pending.
Root owns final handoff; Git is on main. Do not restart completed tasks without
new direction. MMC reads are verified, but vendor filesystem write ownership
remains unestablished. Sudo expires at 16:37 UTC on 2026-09-11.

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
