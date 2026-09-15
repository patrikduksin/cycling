# Firmware services

Portable acquisition and persistence mechanisms used by device implementations
and firmware consumers. These modules depend on device contracts, never on a
board, the shell or the cycling application.

- `gps`, `positioning` and `position_control` decode receiver input, publish aged
  observations and track bounded pause/resume operations.
- `ant` owns bounded discovery and channel state. Cycling profile decoding belongs
  to the application.
- `connectivity`, `network` and `network_time` handle reconnect intent, response
  validation and clock synchronization.
- `storage` owns the two-sector journal format. `bulk` validates explicitly owned
  media regions. Neither module owns physical flash addresses.
- `input` owns bounded edge delivery and cancellation after loss. `sound` tracks
  request submission and elapsed time without claiming acoustic acknowledgment.

Use a module's owner object and typed observations. Keep buffers and intermediate
state private. Add a module here only when it is portable mechanism with a clear
owner; device wire formats belong beside that device, and product decisions
belong beside the application or shell.

The crate and its tests run without hardware. Run `mise run test` for the
workspace's host checks. Tests exercise loss, stale observations, bounded queues
and interrupted persistence because those guarantees must survive extraction
and reuse.
