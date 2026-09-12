# cycling

The approved autonomous issue queue and execution agreement are in
`docs/overnight-plan.md`.

Keep the workspace small. Firmware lives in `packages/os`; device research lives in
`packages/stock/<device>`. Use mise tasks and commit the Cargo lockfile.

Run `mise run test`, `mise run check`, and `mise run build` for firmware changes.
Run both base and cycling SDK checks through the mise tasks. The legacy renderer
and preview task were removed by refactor #64.

For the C606, preserve stock ota_0, the bootloader, partition table and eFuses.
Use `mise run backup`, `mise run flash`, and `mise run stock`; generic flash
commands may replace the bootloader or stock slot. Validate on hardware when
changing display timing or device access. Report what was actually observed.

Publish our code, tools and written findings. Keep vendor firmware, disassembly,
flash dumps, credentials, device identifiers and capture logs in ignored `.local/`.
Distinguish verified hardware from driver candidates supported by stock firmware.

## Device test controls

`mise run build` and `mise run flash` enable bounded diagnostic fault controls by
default. `CYCLING_HARNESS=0` removes those controls; ordinary terminal commands,
JSON logging, Wi-Fi and the public HTTP bring-up check remain available. The
legacy UI injection/capture/recording buffers were removed by refactor #64.
Use `CYCLING_SDK=1` for the optional cycling composition; the default base has no
cycling code. Build both harness modes for feature-boundary changes and restore
a harness-enabled base on the development device after testing.

Read `docs/device-debugging.md` before device tests. Keep build mode and collection
state with observations. Use harness-disabled builds for performance/power
baselines. Serial commands and successful LCD transfers do not prove physical
switches, touch accuracy, panel output or battery hardware. Keep raw evidence,
coordinates, sensor readings and identifiers in ignored `.local/`. Distinguish
sampled heap minima from peak memory and event timing from power measurements.
