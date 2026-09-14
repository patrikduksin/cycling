//! N22 finite companion patterns. Other indices may change configuration or
//! fail to terminate, so they are never accepted through this interface.
use cycling_os::{companion, sound::Pattern};
pub const PATTERNS: &[Pattern] = &[
    Pattern {
        id: 0,
        frequency_hz: 3000,
        nominal_ms: 75,
        guard_ms: 500,
    },
    Pattern {
        id: 10,
        frequency_hz: 4000,
        nominal_ms: 100,
        guard_ms: 500,
    },
    Pattern {
        id: 21,
        frequency_hz: 2500,
        nominal_ms: 150,
        guard_ms: 500,
    },
    Pattern {
        id: 22,
        frequency_hz: 3000,
        nominal_ms: 300,
        guard_ms: 700,
    },
];
pub fn encode(pattern: Option<u8>) -> Option<[u8; 16]> {
    let id = match pattern {
        None => 19,
        Some(id) if PATTERNS.iter().any(|p| p.id == id) => id,
        _ => return None,
    };
    Some(companion::command(16, [0xe2, 1, id, 0, 0, 0, 0, 0]))
}
