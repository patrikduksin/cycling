# Firmware shell

The `firmware-shell` crate owns shared foreground presentation and input policy.
It accepts independent display, input, power and owned-storage implementations,
so its source can run on C606 and in host simulation.

Start with `src/shell.rs` for `Shell` and input routing. `idle.rs` owns inactivity
and wake behavior, `rendering/` owns shared drawing, `preferences.rs` owns the
persisted settings format, and `storage.rs` connects settings to the journal
mechanism. `harness.rs` provides the shell's diagnostic state and controls.

The application runtime remains in VANA. Composition coordinates it with the
shell; there is no generic application framework. Keep device imports and cycling
profile interpretation out of this crate.

Settings version 6 adds a small application area. The shell bounds its length
and carries its bytes through every save without interpreting them; the
application owns their schema, versioning and recovery (VANA stores its training
profile and saved ANT sensors there, see `apps/vana/src/app/preferences.rs`).

The settings format includes the BLE profile selector for compatibility. Retain
its bytes and migration behavior even though VANA interprets the selection; moving
it requires an explicit format migration. Public settings and diagnostic fields
remain integration points; prefer operations that keep their invariants with the
shell when changing those interfaces.

Run `mise run test` for host checks. The simulator exercises the same shell with
multiple display sizes, scripted input and storage failures. It cannot validate
physical display timing or power transitions.
