# Shell simulator

The `shell-simulator` package runs the shared shell and optional VANA application
on a host. It owns its fake device implementations because they serve simulation
and test control rather than physical hardware.

Start with `src/main.rs` for host composition. `devices.rs` supplies framebuffer,
input, power and memory implementations. `session.rs` adds terminal sessions and fixtures. `storage.rs` owns file-backed
reservations, exclusive access and injected write failures. `tests/` covers portability,
virtual sessions and diagnostic controls.

Run `mise run simulate` for the host demonstration or `mise run harness-virtual`
for shared scenarios. Terminal mode requires an explicit disposable state
directory; consult the task and CLI definitions for options.

Simulation checks presentation, input routing, command compatibility and recovery
from controlled storage failures. It does not model DMA timing, radio coexistence,
electrical behavior or power sequencing. Device changes still need the relevant
hardware validation.
