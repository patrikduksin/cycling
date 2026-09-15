# VANA

The `vana` crate owns the cycling application. Keep its screens and domain behavior
in one package so a ride workflow can change without coordinating artificial
library boundaries.

Start with `src/app.rs`. Its `Runtime` handles input, advances recording and sensor
state, presents screens through the shell, and handles application commands.
Firmware and simulator composition call it directly. The private `src/app/`
modules group navigation, recording, presentation and command handling; they
implement the same runtime without exposing its state.

- `src/ride/` owns journal records, metrics, recording, recovery and explicit reclaim.
- `src/sensors/` owns BLE and ANT profile interpretation, selection and radar behavior.
- `src/screens/` owns workout and sensor presentation.

VANA consumes device contracts and the shared shell. It must not import a board or
HAL. Hardware acquisition continues independently of the active screen.

Preserve existing ride records and terminal wire formats when changing these
modules. The shell settings envelope currently stores the BLE profile selector;
VANA interprets it. Moving that field requires a compatibility plan.

Run the host checks through `mise run test`. See [ride recording](../../docs/ride-recording.md)
for persistence and export guarantees, and [architecture](../../docs/architecture.md)
for package boundaries.
