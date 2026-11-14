# Local Trouble Host patch

This directory contains the published `trouble-host` 0.7.0 crate, sourced from
crates.io (upstream commit `e1aff18291e466474752a3beea75004325e6e0ad`).
It remains under the upstream Apache-2.0 OR MIT license.

The local change prevents Read By Type and Read By Group Type responses from
adding a later attribute when that complete entry does not fit in the negotiated
ATT response. Trouble Host truncates the value before comparing entry lengths.
At ATT MTU 23, two short characteristic declarations followed by a 128-bit
declaration could therefore expose the first two bytes of the long UUID as a
false short UUID. The tests in `src/attribute_server.rs` cover the minimum MTU
response and pagination to the complete long declaration.

Upstream status, checked 2026-11-14: releases 0.7.0 and 0.8.0 and `main` at
`042bd1c3bdc5980f66d28e0e698b6e579b15da4e` still compare truncated lengths.
Neither embassy-rs/trouble#546 (closed, the outer `process_accept` truncation)
nor the open embassy-rs/trouble#616 (a response size cap for Windows) changes
that comparison. 0.7.0 is the newest release on bt-hci 0.9, the version
esp-radio 1.0.0-beta.1 implements.

The change lives in `src/attribute_server.rs` only: `full_value_len` gives the
complete value length, reading a per-client value from the client attribute
tables, and both handlers stop before an entry whose complete length differs
from the first entry's or does not fit. The manifest replaces `resolver = "2"`
with an empty `[workspace]`, so the crate builds outside the cycling workspace.
