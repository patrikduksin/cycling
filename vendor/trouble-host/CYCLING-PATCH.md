# Local Trouble Host patch

This directory contains the published `trouble-host` 0.6.0 crate, sourced from
crates.io (upstream commit `29b0c515831e4be2bba5e55616bfa6297d75db31`).
It remains under the upstream Apache-2.0 OR MIT license.

The local change prevents Read By Type and Read By Group Type responses from
adding a later attribute when that complete entry does not fit in the negotiated
ATT response. Trouble Host previously truncated the value before comparing entry
lengths. At ATT MTU 23, two short characteristic declarations followed by a
128-bit declaration could therefore expose the first two bytes of the long UUID
as a false short UUID. The tests in `src/attribute_server.rs` cover the minimum
MTU response and pagination to the complete long declaration.
