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
- Starting #1. No implementation changes yet.

Update this section after each issue with branch/PR, verification, review outcome,
and the next task. Keep raw evidence in .local/ and link written results in PRs.
