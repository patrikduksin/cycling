//! The script tiles VANA's build compiles from `apps/vana/guests`.

/// The spike's guest: its countdown, graph and runaway loop on the `bench`
/// host.
pub const BENCH: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bench.wasm"));
/// A ring over each hour of the ride around its minutes.
pub const COUNTDOWN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/countdown.wasm"));
/// The last minute of power, drawn by `series`.
pub const TREND: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/trend.wasm"));
/// Loops for ever at one speed the trial readings miss.
pub const RUNAWAY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/runaway.wasm"));
/// Loops for ever on every draw.
pub const SPIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spin.wasm"));
/// Draws past its box.
pub const OUTSIDE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/outside.wasm"));
/// Draws past its box at the top of the power range.
pub const WIDE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wide.wasm"));
/// Spins in `__post_instantiate`.
pub const CTOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ctor.wasm"));
/// Right-aligns text in a box narrower than a glyph's spill.
pub const RIGHTEDGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rightedge.wasm"));
/// Measures the empty text at the end of its memory.
pub const NULLSLICE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nullslice.wasm"));
/// Measures a 60,000-byte text in a loop.
pub const WIDTHS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/widths.wasm"));
/// Fills 60,000 bytes in a loop.
pub const FILL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fill.wasm"));
/// Draws a ring mostly past its box's top-left corner.
pub const RINGLEFT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ringleft.wasm"));
/// Draws a series into a box with no height (look 0) or no width (look 1).
pub const FLAT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flat.wasm"));
/// Calls the native its look names over and over.
pub const CHATTY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/chatty.wasm"));
