//! Bytes a test tap folds into its digests, in the trace's own codes, so a
//! rename of a Rust type changes no digest. Each value is encoded alone and
//! whole, however large.
use super::{Event, Medium, StorageError, StorageOp, Wire, Writer};
use std::vec::Vec;

fn grown(put: impl Fn(&mut Writer)) -> Vec<u8> {
    let mut buf = std::vec![0u8; 256];
    loop {
        let mut w = Writer::new(&mut buf, 0);
        put(&mut w);
        if !w.overflow {
            let len = w.len;
            buf.truncate(len);
            return buf;
        }
        let doubled = buf.len() * 2;
        buf.resize(doubled, 0);
    }
}

/// The event as a block records it, its pass clock absolute.
pub fn event(event: &Event<'_>) -> Vec<u8> {
    grown(|w| event.put_record(w, 0))
}

pub fn storage(
    medium: Medium,
    op: StorageOp,
    at: u64,
    len: u64,
    result: Result<(), StorageError>,
) -> Vec<u8> {
    grown(|w| {
        medium.put(w);
        op.put(w);
        w.varint(at);
        w.varint(len);
        match result {
            Ok(()) => w.byte(0),
            Err(error) => {
                w.byte(1);
                error.put(w);
            }
        }
    })
}
