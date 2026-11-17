use super::*;
use crate::testing::Rng;
use std::vec;
use std::vec::Vec;

fn elided(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bytes.len() + 16];
    let mut w = Writer::new(&mut out, 0);
    elide(&mut w, bytes);
    assert!(!w.overflow);
    let len = w.len;
    out.truncate(len);
    out
}

fn expanded(runs: &[u8], len: usize) -> Option<Vec<u8>> {
    let access = Access {
        span: Span {
            medium: Medium::Bulk,
            start: 0,
            count: 1,
        },
        content: Content::Inline(runs),
        outside: false,
        fault: None,
    };
    let mut out = vec![0x55u8; len];
    let n = access.expand(&mut out)?;
    out.truncate(n);
    Some(out)
}

#[test]
fn a_blank_sector_is_two_bytes_and_a_short_run_stays_literal() {
    assert_eq!(elided(&[0xff; 512]), [0x81, 0x10]);
    assert_eq!(elided(&[0x00; 512]), [0x82, 0x10]);
    assert_eq!(
        elided(&[0xff; 7]),
        [7 << 2, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
    );
    assert_eq!(elided(&[]), [0u8; 0]);
}

#[test]
fn elided_bytes_expand_to_what_they_were() {
    let mut rng = Rng::new(7);
    for _ in 0..2_000 {
        let mut bytes = Vec::new();
        for _ in 0..rng.below(6) {
            let len = rng.below(40) as usize;
            match rng.below(3) {
                0 => bytes.extend(core::iter::repeat_n(0xff, len)),
                1 => bytes.extend(core::iter::repeat_n(0x00, len)),
                _ => bytes.extend((0..len).map(|_| rng.byte())),
            }
        }
        let runs = elided(&bytes);
        assert!(runs.len() <= bytes.len() + 4, "{bytes:?}");
        assert_eq!(
            expanded(&runs, bytes.len()),
            Some(bytes.clone()),
            "{bytes:?}"
        );
        if !bytes.is_empty() {
            assert_eq!(
                expanded(&runs, bytes.len() - 1),
                None,
                "a short buffer is refused"
            );
        }
    }
}

#[test]
fn the_card_is_classed_by_its_volumes() {
    let extent = |start, sectors| {
        Some(Extent {
            name: volume::STORE,
            start,
            sectors,
        })
    };
    let volumes = Volumes {
        terrain: extent(100, 50),
        store: extent(150, 10),
    };
    let class = |sector| Place::card(&volumes, sector, 512).class;
    assert_eq!(class(99), Class::Inline);
    assert_eq!(
        (class(100), class(149)),
        (Class::DigestOnly, Class::DigestOnly)
    );
    assert_eq!((class(150), class(159)), (Class::Volatile, Class::Volatile));
    assert_eq!(class(160), Class::Inline);
    assert_eq!(class(u64::from(u32::MAX) + 1), Class::Outside);
    assert_eq!(Volumes::default().class(5), Class::Inline);
}

#[test]
fn the_terrain_packs_headers_are_carried_and_its_data_is_not() {
    let mut card = crate::testing::Media::new(5_777_407);
    let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
    assert_eq!(Volumes::read(&mut card, &mut sector), Volumes::default());
    let table = volume::create(&mut card, &mut sector).unwrap();
    let volumes = Volumes::read(&mut card, &mut sector);
    let pack = table.find(&volume::TERRAIN).unwrap();
    let class = |sector| volumes.class(sector);
    assert_eq!(class(pack.start), Class::Inline);
    assert_eq!(class(pack.start + crate::pack::DATA - 1), Class::Inline);
    assert_eq!(class(pack.start + crate::pack::DATA), Class::DigestOnly);
    assert_eq!(class(pack.start + pack.sectors - 1), Class::DigestOnly);
    assert_eq!(volumes.store, table.find(&volume::STORE));
}
