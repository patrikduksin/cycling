//! Experiment: keep the last non-report companion frames for inspection.
use core::cell::RefCell;
use critical_section::Mutex;

const KEPT: usize = 8;

#[derive(Clone, Copy)]
struct Seen {
    at_ms: u64,
    len: u8,
    head: [u8; 24],
}

struct Ring {
    seen: [Option<Seen>; KEPT],
    next: usize,
    total: u32,
}

static RING: Mutex<RefCell<Ring>> = Mutex::new(RefCell::new(Ring {
    seen: [None; KEPT],
    next: 0,
    total: 0,
}));

pub fn observe(bytes: &[u8], now: u64) {
    if bytes.len() > 4 && bytes[4] == 4 {
        return;
    }
    let mut head = [0u8; 24];
    let n = bytes.len().min(24);
    head[..n].copy_from_slice(&bytes[..n]);
    critical_section::with(|cs| {
        let mut ring = RING.borrow_ref_mut(cs);
        let next = ring.next;
        ring.seen[next] = Some(Seen {
            at_ms: now,
            len: bytes.len().min(255) as u8,
            head,
        });
        ring.next = (next + 1) % KEPT;
        ring.total = ring.total.wrapping_add(1);
    });
}

pub fn write(output: &mut impl core::fmt::Write) {
    critical_section::with(|cs| {
        let ring = RING.borrow_ref(cs);
        let _ = write!(output, "total={}", ring.total);
        for i in 0..KEPT {
            if let Some(seen) = ring.seen[(ring.next + i) % KEPT] {
                let _ = write!(output, " {}ms/{}:", seen.at_ms, seen.len);
                for b in &seen.head[..usize::from(seen.len).min(24)] {
                    let _ = write!(output, "{:02x}", b);
                }
            }
        }
    });
}
