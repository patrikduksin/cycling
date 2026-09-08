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
