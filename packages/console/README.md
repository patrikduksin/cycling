# Firmware console

The `firmware-console` crate owns terminal sessions and shared command handling.
Start with `src/session.rs` for `Terminal`, `protocol.rs` for request and response
types, and `commands/` for shell and peripheral operations. `log_record.rs` defines
bounded log records.

The console consumes device contracts and the shell. VANA owns application
command behavior, and composition joins the handlers. Do not import a concrete
device or move ride policy into terminal parsing.

Preserve command names, reply framing and completion semantics. A request accepted
by a device can still be pending; formatting a successful submission must not
claim that the operation completed. Keep logs and replies bounded so a missing
host cannot stop acquisition or consume unbounded memory.

Run `mise run test` for host checks. Shared session scenarios also run through
`mise run harness-virtual`; see [testing harness](../../docs/testing-harness.md).
