use super::*;
use crate::sector_log::{self, Recovery, Step};
use crate::testing::{Media, Tear};

const C606: u64 = 5_777_407;
const TAG: &[u8; 16] = b"TEST-LOG-SCHEMA1";

fn sector() -> Sector {
    [0; SECTOR_SIZE]
}

#[test]
fn the_default_plan_fits_the_c606_region_in_disjoint_aligned_volumes() {
    let table = plan(C606).unwrap();
    let rides = table.find(&RIDES).unwrap();
    let files = table.find(&FILES).unwrap();
    let terrain = table.find(&TERRAIN).unwrap();
    assert_eq!((rides.start, rides.sectors), (2048, 2_097_152));
    assert_eq!(files.start, rides.start + rides.sectors);
    assert_eq!(terrain.start, files.start + files.sectors);
    assert_eq!(terrain.sectors * 512, 1280 << 20);
    for volume in table.volumes() {
        assert_eq!(volume.start % ALIGN, 0, "{}", volume.label());
    }
    // STORE takes the last 260 MiB.
    let store = table.find(&STORE).unwrap();
    assert_eq!(
        (store.start, store.sectors, store.kind),
        (terrain.start + terrain.sectors, 532_479, Kind::Store)
    );
    assert_eq!(table.unallocated(C606), 0);
    // About 2,000 hours of one-record-per-second rides.
    assert!(rides.sectors / 3600 > 580);
}

/// The table every C606 got before STORE: the default plan's first three
/// volumes, in both copies.
fn table_before_store(media: &mut Media) -> Table {
    let mut table = plan(C606).unwrap();
    table.volumes[3] = None;
    let mut s = sector();
    table.encode(&mut s);
    media.set(0, s);
    media.set(1, s);
    table
}

/// Rides, a file and the terrain pack's header as an owner's device holds
/// them, so a migration can be checked not to touch them.
fn fill_volumes(media: &mut Media) {
    let mut s = sector();
    let mut rides = open(media, &RIDES, &mut s).unwrap();
    let mut log = sector_log::format(&mut rides, TAG, 9, b"cat", &mut s).unwrap();
    for _ in 0..3 {
        log.append(&mut rides, b"ride", &mut s).unwrap();
    }
    let mut files = open(media, &FILES, &mut s).unwrap();
    crate::files::format(&mut files, 1, &mut s).unwrap();
    let name = crate::files::Name::new("course.dat").unwrap();
    crate::files::write(&mut files, &name, b"course", &mut s).unwrap();
    let terrain = plan(C606).unwrap().find(&TERRAIN).unwrap();
    media.set(terrain.start, [0x7e; SECTOR_SIZE]);
}

#[test]
fn a_table_from_before_store_gains_it_only_in_the_unallocated_tail() {
    let mut media = Media::new(C606);
    let mut s = sector();
    let old = table_before_store(&mut media);
    fill_volumes(&mut media);
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Table(old)));
    assert_eq!(open(&mut media, &STORE, &mut s).err(), Some(Error::Missing));
    // Opening volumes and initializing never add it.
    assert_eq!(create(&mut media, &mut s), Ok(old));
    let before = media.written.clone();
    let writes = media.writes;

    let migrated = add_store(&mut media, &mut s).unwrap();
    assert_eq!(media.writes - writes, 2, "both table copies, nothing else");
    assert_eq!(migrated.generation, old.generation + 1);
    assert_eq!(migrated.planned, old.planned);
    for volume in old.volumes() {
        assert_eq!(
            migrated.find(&volume.name),
            Some(*volume),
            "{}",
            volume.label()
        );
    }
    let store = migrated.find(&STORE).unwrap();
    assert_eq!((store.start, store.sectors), (5_244_928, 532_479));
    assert_eq!(
        migrated,
        plan(C606).map(|t| Table { generation: 2, ..t }).unwrap()
    );
    for (at, bytes) in &before {
        if *at >= TABLE {
            assert_eq!(media.get(*at), *bytes, "sector {at} changed");
        }
    }
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Table(migrated)));
    let mut rides = open(&mut media, &RIDES, &mut s).unwrap();
    let mut recovery = Recovery::new(TAG, 32);
    let head = loop {
        match recovery.step(&mut rides, &mut s).unwrap() {
            Step::Done(log) => break log.head(),
            Step::Uncertain(_) => panic!("uncertain"),
            _ => {}
        }
    };
    assert_eq!(head, 3, "the rides are still there");
    let mut files = open(&mut media, &FILES, &mut s).unwrap();
    let name = crate::files::Name::new("course.dat").unwrap();
    let dir = crate::files::mount(&mut files, &mut s).unwrap();
    assert!(dir.find(&mut files, &name, &mut s).unwrap().is_some());
    let volume = open(&mut media, &STORE, &mut s).unwrap();
    assert_eq!(volume.extent(), store);

    // Already migrated: nothing to write, the same table.
    let writes = media.writes;
    assert_eq!(add_store(&mut media, &mut s), Ok(migrated));
    assert_eq!(media.writes, writes);
    // A fresh table already has it.
    let mut fresh = Media::new(C606);
    let table = create(&mut fresh, &mut s).unwrap();
    let writes = fresh.writes;
    assert_eq!(add_store(&mut fresh, &mut s), Ok(table));
    assert_eq!(fresh.writes, writes);
}

#[test]
fn a_cut_while_adding_store_leaves_the_old_or_new_table_and_running_again_completes_it() {
    for at in 0..2 {
        for tear in [Tear::Old, Tear::Prefix(100), Tear::Prefix(511), Tear::New] {
            let mut media = Media::new(C606);
            let mut s = sector();
            let old = table_before_store(&mut media);
            fill_volumes(&mut media);
            let before = media.written.clone();
            media.cut = Some((at, tear));
            assert!(add_store(&mut media, &mut s).is_err());
            media.reboot();
            let new = Table {
                generation: 2,
                ..plan(C606).unwrap()
            };
            match read(&mut media, &mut s) {
                Ok(Layout::Table(table)) => assert!(
                    table == old || table == new,
                    "cut at {at} {tear:?}: {table:?}"
                ),
                other => panic!("cut at {at} {tear:?}: {other:?}"),
            }
            assert_eq!(add_store(&mut media, &mut s), Ok(new));
            assert_eq!(media.get(0), media.get(1), "both copies hold it");
            assert_eq!(read(&mut media, &mut s), Ok(Layout::Table(new)));
            for (sector, bytes) in &before {
                if *sector >= TABLE {
                    assert_eq!(media.get(*sector), *bytes);
                }
            }
        }
    }
}

#[test]
fn without_a_table_this_version_understands_adding_store_writes_nothing() {
    let mut s = sector();
    // Blank.
    let mut media = Media::new(C606);
    assert_eq!(add_store(&mut media, &mut s), Err(Error::Missing));
    assert_eq!(media.writes, 0);
    // The ride log from before volumes.
    let mut log = sector_log::format(&mut media, TAG, 7, b"catalog", &mut s).unwrap();
    log.append(&mut media, b"old test ride", &mut s).unwrap();
    let writes = media.writes;
    assert_eq!(add_store(&mut media, &mut s), Err(Error::Missing));
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Legacy));
    assert_eq!(media.writes, writes);
    // A lost table over a ride log.
    let mut media = Media::new(C606);
    table_before_store(&mut media);
    fill_volumes(&mut media);
    media.set(0, [0xa5; SECTOR_SIZE]);
    media.set(1, [0; SECTOR_SIZE]);
    let writes = media.writes;
    assert_eq!(add_store(&mut media, &mut s), Err(Error::Missing));
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Lost));
    assert_eq!(media.writes, writes);
    // A foreign table, and a STORE this version did not write.
    let mut media = Media::new(C606);
    let old = table_before_store(&mut media);
    let good = media.get(0);
    let mut newer = good;
    newer[16] = 2;
    let checksum = crc32(&newer[..CRC]);
    newer[CRC..].copy_from_slice(&checksum.to_le_bytes());
    media.set(0, newer);
    assert_eq!(add_store(&mut media, &mut s), Err(Error::Foreign));
    let mut odd = old;
    odd.volumes[3] = Some(Extent {
        kind: Kind::Files,
        ..plan(C606).unwrap().find(&STORE).unwrap()
    });
    odd.encode(&mut s);
    media.set(0, s);
    media.set(1, s);
    assert_eq!(add_store(&mut media, &mut s), Err(Error::Foreign));
    assert_eq!(media.writes, 0);
    // No room: small media give rides everything.
    let mut small = Media::new(100);
    create(&mut small, &mut s).unwrap();
    let writes = small.writes;
    assert_eq!(add_store(&mut small, &mut s), Err(Error::Small));
    assert_eq!(small.writes, writes);
}

#[test]
fn small_regions_give_rides_everything_after_the_table() {
    let table = plan(100).unwrap();
    assert_eq!(table.volumes().count(), 1);
    assert_eq!(table.find(&RIDES).unwrap().start, TABLE);
    assert_eq!(table.find(&RIDES).unwrap().sectors, 98);
    assert_eq!(plan(2), Err(Error::Small));
}

#[test]
fn create_writes_two_sectors_only_over_a_blank_region_and_is_idempotent() {
    let mut media = Media::new(C606);
    let mut s = sector();
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Blank));
    let table = create(&mut media, &mut s).unwrap();
    assert_eq!(media.writes, 2);
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Table(table)));
    assert_eq!(create(&mut media, &mut s), Ok(table));
    assert_eq!(media.writes, 2);
    // One damaged copy is enough.
    media.set(0, [0xa5; SECTOR_SIZE]);
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Table(table)));
}

#[test]
fn a_cut_while_creating_leaves_a_whole_table_or_the_old_region() {
    for at in 0..2 {
        for tear in [Tear::Old, Tear::Prefix(100), Tear::Prefix(511), Tear::New] {
            let mut media = Media::new(C606);
            let mut s = sector();
            media.cut = Some((at, tear));
            assert!(create(&mut media, &mut s).is_err());
            media.reboot();
            match read(&mut media, &mut s).unwrap() {
                Layout::Blank => {}
                Layout::Table(table) => assert_eq!(table, plan(C606).unwrap()),
                Layout::Legacy | Layout::Lost => panic!("never legacy or lost"),
            }
        }
    }
}

#[test]
fn the_legacy_whole_region_log_is_reported_and_never_read_as_a_volume() {
    let mut media = Media::new(C606);
    let mut s = sector();
    let mut log = sector_log::format(&mut media, TAG, 7, b"catalog", &mut s).unwrap();
    for _ in 0..40 {
        log.append(&mut media, b"old test ride", &mut s).unwrap();
    }
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Legacy));
    // Opening a volume of a legacy region fails rather than guessing.
    assert_eq!(open(&mut media, &RIDES, &mut s).err(), Some(Error::Missing));
    // Only one superblock left is still legacy.
    media.set(0, [0; SECTOR_SIZE]);
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Legacy));
    // An explicit create replaces it; the old records stay unreachable.
    let table = create(&mut media, &mut s).unwrap();
    let mut rides = open(&mut media, &RIDES, &mut s).unwrap();
    assert_eq!(rides.extent(), table.find(&RIDES).unwrap());
    let mut recovery = Recovery::new(TAG, 32);
    let outcome = loop {
        match recovery.step(&mut rides, &mut s) {
            Ok(Step::More) => {}
            other => break other,
        }
    };
    assert_eq!(outcome.err(), Some(sector_log::Error::Uninitialized));
}

#[test]
fn tables_this_version_cannot_use_are_foreign_and_never_replaced() {
    let mut media = Media::new(C606);
    let mut s = sector();
    create(&mut media, &mut s).unwrap();
    let good = media.get(0);
    // Semantic damage resealed, so only the checks below can catch it.
    let reseal = |offset: usize, value: u8| {
        let mut changed = good;
        changed[offset] = value;
        let checksum = crc32(&changed[..CRC]);
        changed[CRC..].copy_from_slice(&checksum.to_le_bytes());
        changed
    };
    for (offset, value) in [
        (16, 2),                       // version
        (21, 0),                       // sector size
        (18, 9),                       // count beyond the maximum
        (ENTRIES + 8, 9),              // unknown kind
        (ENTRIES, 0),                  // unnamed
        (ENTRIES + 10, 1),             // reserved entry bytes
        (ENTRIES + ENTRY + 17, 0),     // second volume overlaps the first
        (ENTRIES + 2 * ENTRY + 31, 1), // third volume past the region
        (300, 1),                      // bytes after the last entry
    ] {
        media.set(0, reseal(offset, value));
        media.set(1, good);
        let writes = media.writes;
        assert_eq!(read(&mut media, &mut s), Err(Error::Foreign), "{offset}");
        assert_eq!(create(&mut media, &mut s), Err(Error::Foreign), "{offset}");
        assert_eq!(media.writes, writes);
    }
    // A region that shrank under its table is foreign too.
    media.set(0, good);
    media.sectors = 5_000_000;
    assert_eq!(read(&mut media, &mut s), Err(Error::Foreign));
    // Duplicate names.
    media.sectors = C606;
    let mut changed = good;
    changed.copy_within(ENTRIES..ENTRIES + 8, ENTRIES + ENTRY);
    let checksum = crc32(&changed[..CRC]);
    changed[CRC..].copy_from_slice(&checksum.to_le_bytes());
    media.set(0, changed);
    assert_eq!(read(&mut media, &mut s), Err(Error::Foreign));
}

#[test]
fn the_newest_generation_wins() {
    let mut media = Media::new(C606);
    let mut s = sector();
    let mut table = create(&mut media, &mut s).unwrap();
    table.generation = 2;
    table.volumes[2].as_mut().unwrap().sectors -= ALIGN;
    table.encode(&mut s);
    media.set(1, s);
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Table(table)));
}

#[test]
fn a_volume_is_its_own_device_and_cannot_reach_its_neighbours() {
    let mut media = Media::new(C606);
    let mut s = sector();
    let table = create(&mut media, &mut s).unwrap();
    let files = table.find(&FILES).unwrap();
    let mut volume = Volume::new(&mut media, files);
    let info = volume.owned_info().unwrap();
    assert_eq!(
        (info.start_sector, info.sectors),
        (1000 + files.start, files.sectors)
    );
    volume.owned_write(0, &[1; SECTOR_SIZE]).unwrap();
    volume
        .owned_write(files.sectors - 1, &[2; SECTOR_SIZE])
        .unwrap();
    assert_eq!(
        volume.owned_write(files.sectors, &[3; SECTOR_SIZE]),
        Err(MediaError::Range)
    );
    assert_eq!(volume.owned_read(u64::MAX, &mut s), Err(MediaError::Range));
    assert_eq!(media.get(files.start), [1; SECTOR_SIZE]);
    assert_eq!(media.get(files.start + files.sectors - 1), [2; SECTOR_SIZE]);
    assert_eq!(media.writes, 4);
}

#[test]
fn the_sector_log_runs_unchanged_inside_its_volume() {
    let mut media = Media::new(C606);
    let mut s = sector();
    create(&mut media, &mut s).unwrap();
    let mut rides = open(&mut media, &RIDES, &mut s).unwrap();
    let mut log = sector_log::format(&mut rides, TAG, 9, b"cat", &mut s).unwrap();
    assert_eq!(log.capacity(), RIDES_SECTORS - sector_log::DATA_START);
    for _ in 0..5 {
        log.append(&mut rides, b"ride", &mut s).unwrap();
    }
    log.checkpoint(&mut rides, b"cat", &mut s).unwrap();
    let mut recovery = Recovery::new(TAG, 32);
    let head = loop {
        match recovery.step(&mut rides, &mut s).unwrap() {
            Step::Done(log) => break log.head(),
            Step::Uncertain(_) => panic!("uncertain"),
            _ => {}
        }
    };
    assert_eq!(head, 5);
    // Nothing outside RIDES and the table was written.
    let rides = plan(C606).unwrap().find(&RIDES).unwrap();
    assert!(
        media
            .written
            .keys()
            .all(|k| *k < TABLE || (rides.start..rides.start + rides.sectors).contains(k))
    );
}

#[test]
fn a_lost_table_over_a_ride_log_is_not_blank_and_create_restores_it() {
    let mut media = Media::new(C606);
    let mut s = sector();
    let table = create(&mut media, &mut s).unwrap();
    let mut rides = open(&mut media, &RIDES, &mut s).unwrap();
    let mut log = sector_log::format(&mut rides, TAG, 9, b"cat", &mut s).unwrap();
    log.append(&mut rides, b"ride", &mut s).unwrap();
    media.set(0, [0xa5; SECTOR_SIZE]);
    media.set(1, [0; SECTOR_SIZE]);
    assert_eq!(read(&mut media, &mut s), Ok(Layout::Lost));
    assert_eq!(create(&mut media, &mut s), Ok(table));
    let mut rides = open(&mut media, &RIDES, &mut s).unwrap();
    assert_eq!(log.read(&mut rides, 0, &mut s).unwrap(), Some(&b"ride"[..]));
    // Small media have no other plan to check.
    let mut small = Media::new(100);
    assert_eq!(read(&mut small, &mut s), Ok(Layout::Blank));
}
