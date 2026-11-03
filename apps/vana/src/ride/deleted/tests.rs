use super::*;
use crate::workout::library::tests::{formatted, put};

const STORE: u64 = 0x5eed;

fn ride(ride_id: u32) -> Ride {
    Ride::new(b'b', STORE, ride_id, Some(1_760_000_000_000), 600_000)
}

fn record(ride_id: u32) -> Record {
    Record {
        ride: ride(ride_id),
        reason: Reason::Deleted,
        hidden_utc_s: 1_760_000_900,
    }
}

fn shelf() -> Shelf {
    Shelf::new(std::vec![0u8; SHELF_BYTES].leak())
}

fn read_back(card: &mut impl ReadWrite) -> Shelf {
    let mut shelf = shelf();
    read(card, &mut shelf).unwrap();
    shelf
}

#[test]
fn a_record_round_trips_and_a_damaged_slot_is_none() {
    let saved = Record {
        reason: Reason::Discarded,
        ..record(7)
    };
    let mut slot = encode(&saved);
    assert_eq!(decode(&slot), Some(saved));
    slot[5] ^= 1;
    assert_eq!(decode(&slot), None);
}

#[test]
fn the_key_holds_without_a_clock() {
    let undated = Ride::new(b'i', 0, 3, None, 12_999);
    assert_eq!(undated.started_utc_s, 0);
    assert_eq!(undated.ride_s, 12);
    let mut card = formatted();
    let mut kept = shelf();
    let hidden = Record {
        ride: undated,
        ..record(3)
    };
    save(&mut card, &mut kept, &hidden).unwrap();
    let back = read_back(&mut card);
    assert!(back.hides(&undated));
    assert!(
        !back.hides(&Ride::new(b'i', 0, 3, None, 13_000)),
        "ride time differs"
    );
    assert!(
        !back.hides(&Ride::new(b'b', 0, 3, None, 12_999)),
        "other journal"
    );
}

#[test]
fn saves_persist_and_a_second_save_writes_nothing() {
    let mut card = formatted();
    let mut kept = shelf();
    assert!(!read(&mut card, &mut kept).unwrap(), "no table yet");
    for id in [5, 2, 9, 2] {
        save(&mut card, &mut kept, &record(id)).unwrap();
    }
    assert_eq!(kept.len(), 3);
    let back = read_back(&mut card);
    let ids: std::vec::Vec<u32> = back.records().map(|r| r.ride.ride_id).collect();
    assert_eq!(ids, [2, 5, 9]);
    assert!(back.hides(&ride(5)));
    assert!(!back.hides(&ride(6)));
    let other_store = Ride {
        store: STORE + 1,
        ..ride(5)
    };
    assert!(!back.hides(&other_store), "a new log on the card");
}

#[test]
fn forgetting_a_journal_keeps_the_others() {
    let mut card = formatted();
    let mut kept = shelf();
    let internal = Record {
        ride: Ride::new(b'i', 0, 4, None, 1_000),
        ..record(4)
    };
    save(&mut card, &mut kept, &record(4)).unwrap();
    save(&mut card, &mut kept, &internal).unwrap();
    forget(&mut card, &mut kept, b'b').unwrap();
    for shelf in [&kept, &read_back(&mut card)] {
        assert!(!shelf.hides(&ride(4)));
        assert!(shelf.hides(&internal.ride));
    }
}

#[test]
fn a_table_spanning_sectors_reads_whole() {
    let mut card = formatted();
    let mut kept = shelf();
    for id in 1..=100 {
        save(&mut card, &mut kept, &record(id)).unwrap();
    }
    let back = read_back(&mut card);
    assert_eq!(back.len(), 100);
    assert!((1..=100).all(|id| back.hides(&ride(id))));
}

#[test]
fn a_full_table_refuses_the_next_ride() {
    let mut card = formatted();
    let mut kept = Shelf::new(std::vec![0u8; 2 * SLOT].leak());
    save(&mut card, &mut kept, &record(1)).unwrap();
    save(&mut card, &mut kept, &record(2)).unwrap();
    assert_eq!(save(&mut card, &mut kept, &record(3)), Err(Error::Full));
    assert_eq!(read_back(&mut card).len(), 2);
}

#[test]
fn a_foreign_or_later_table_is_read_as_nothing_and_never_written_over() {
    let mut card = formatted();
    put(&mut card, TABLE, b"not a table");
    let mut kept = shelf();
    assert_eq!(read(&mut card, &mut kept), Err(Error::Table));
    assert_eq!(save(&mut card, &mut kept, &record(1)), Err(Error::Table));

    let mut later = std::vec![0u8; 2 * 64];
    later[..4].copy_from_slice(MAGIC);
    later[4] = 2;
    put_u16(&mut later, 6, 64);
    seal(&mut later[..64]);
    put(&mut card, TABLE, &later);
    assert_eq!(read(&mut card, &mut kept), Err(Error::Table));
    assert_eq!(save(&mut card, &mut kept, &record(1)), Err(Error::Table));
    assert_eq!(decode_table(&later).count(), 0);
}

#[test]
fn the_host_decodes_a_whole_table() {
    let mut card = formatted();
    let mut kept = shelf();
    save(&mut card, &mut kept, &record(3)).unwrap();
    let mut table = std::vec![header().to_vec()];
    table.push(encode(&record(3)).to_vec());
    let bytes = table.concat();
    let records: std::vec::Vec<Record> = decode_table(&bytes).collect();
    assert_eq!(records, [record(3)]);
}

fn table_bytes(card: &mut impl ReadWrite) -> std::vec::Vec<u8> {
    let mut sector = [0; SECTOR_SIZE];
    let mut bytes = [0; SECTOR_SIZE];
    let mut store = volume::open(card, &volume::FILES, &mut sector).unwrap();
    let dir = files::mount(&mut store, &mut sector).unwrap();
    let entry = dir
        .find(&mut store, &table_name(), &mut sector)
        .unwrap()
        .unwrap();
    let mut out = std::vec::Vec::new();
    each(
        &mut store,
        &dir,
        &entry,
        &mut sector,
        &mut bytes,
        |_, _, slot| out.extend_from_slice(slot),
    )
    .unwrap();
    out
}

/// A ride trace takes the table back to its length at power-on.
#[test]
fn a_table_that_gains_rides_only_grows_at_its_end() {
    let mut card = formatted();
    let mut kept = shelf();
    for id in [40, 3, 17] {
        save(&mut card, &mut kept, &record(id)).unwrap();
    }
    let before = table_bytes(&mut card);
    save(&mut card, &mut kept, &record(1)).unwrap();
    let after = table_bytes(&mut card);
    assert_eq!(after.len(), before.len() + SLOT);
    assert_eq!(after[..before.len()], before[..]);
    let ids: std::vec::Vec<u32> = decode_table(&after).map(|r| r.ride.ride_id).collect();
    assert_eq!(ids, [40, 3, 17, 1], "file order is save order");
}

/// A table save would refuse hides nothing, so no cue offers a save that
/// cannot happen.
#[test]
fn a_table_of_a_shape_saves_refuse_is_not_read() {
    let mut card = formatted();
    let mut kept = shelf();
    put(&mut card, TABLE, b"");
    assert_eq!(read(&mut card, &mut kept), Err(Error::Table), "empty");
    let mut odd = header().to_vec();
    odd.extend_from_slice(&encode(&record(1)));
    odd.push(0);
    put(&mut card, TABLE, &odd);
    assert_eq!(
        read(&mut card, &mut kept),
        Err(Error::Table),
        "a partial slot"
    );
    assert_eq!(save(&mut card, &mut kept, &record(2)), Err(Error::Table));
}
