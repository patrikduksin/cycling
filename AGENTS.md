# cycling

Keep the workspace small. Firmware lives in `packages/os`; device research lives in
`packages/stock/<device>`. Use mise tasks and commit the Cargo lockfile.

Run `mise run test`, `mise run check`, and `mise run build` for firmware changes.
The renderer also runs on the host through `mise run preview`.

For the C606, preserve stock ota_0, the bootloader, partition table and eFuses.
Use `mise run backup`, `mise run flash`, and `mise run stock`; generic flash
commands may replace the bootloader or stock slot. Validate on hardware when
changing display timing or device access. Report what was actually observed.

Publish our code, tools and written findings. Keep vendor firmware, disassembly,
flash dumps, credentials, device identifiers and capture logs in ignored `.local/`.
Distinguish verified hardware from driver candidates supported by stock firmware.

## Device test harness

`mise run build` and `mise run flash` enable the USB test harness by default.
Use `CYCLING_HARNESS=0 mise run build` or `CYCLING_HARNESS=0 mise run flash`
for firmware without injection, screenshots, recording or their buffers.
When changing the feature boundary, build both modes and validate device access.
Restore a harness-enabled build on the development device after testing.

Read `docs/device-debugging.md` before using the harness or interpreting its
measurements. Recording changes frame timing: observed processing was mostly
20–30 ms, with a 44 ms full-frame capture against a 42 ms frame budget. Requested
capture fps is not achieved fps; use device timestamps. Capture buffers reserve
33.125 KiB even when idle. Idle CPU overhead has not been measured separately.
Use harness-disabled firmware for performance/power baselines, and record the
build mode and recording state with results. Ordinary logs and the Wi-Fi
bring-up test remain enabled in that mode; it is not a quiet production build.
Injected inputs and captured pixels verify firmware behavior, not physical
switches, touch accuracy, panel output or battery hardware. Keep evidence in
ignored `.local/` and distinguish sampled heap minima from peak memory usage.
