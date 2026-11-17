# Device API

Hardware-independent capability contracts and the observations they exchange.
Consumers depend on this package to request work and inspect results without
knowing how a board implements them.

Start with the module named for the capability: `display`, `input`, `power`,
`positioning`, `sensors`, `sound`, `ant`, `ble_transport`, `storage`,
`bulk` or `console`. Each holds its contracts and exchanged types. `observation`
defines shared availability, errors and freshness. `crash` defines the reset-persistent record
format shared by device startup and presentation.

Keep acquisition queues, parsers, connection state machines and hardware drivers
out of this package. A contract must describe completion, unavailable data and
loss where they affect a caller. Do not hide these conditions behind a successful
request submission.

This crate is `no_std`. Run `cycling test` for the workspace's host checks.
