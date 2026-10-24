use super::*;
use crate::testing::{Media, Rng, Tear};
use std::vec::Vec;

fn sector() -> Sector {
    [0; SECTOR_SIZE]
}

fn content(length: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed | 1);
    (0..length).map(|_| rng.next() as u8).collect()
}

fn spec(data: &[u8]) -> Spec {
    Spec {
        length: data.len() as u64,
        crc: crc32(data),
        tag: *b"TEST-TERRAIN-V1\0",
        digest: [7; 32],
        stamp: 1_760_000_000,
    }
}

fn chunk(data: &[u8], index: u64) -> Sector {
    let mut sector = [0u8; SECTOR_SIZE];
    let start = (index * SIZE) as usize;
    let end = data.len().min(start + SECTOR_SIZE);
    sector[..end - start].copy_from_slice(&data[start..end]);
    sector
}

fn install(media: &mut Media, data: &[u8]) -> Header {
    let mut s = sector();
    let mut writer = begin(media, spec(data), &mut s).unwrap();
    for index in 0..writer.header().sectors() {
        writer.write(media, index, &chunk(data, index)).unwrap();
    }
    writer.commit(media, &mut s).unwrap()
}

fn read_all(media: &mut Media) -> Vec<u8> {
    let mut s = sector();
    let pack = open(media, &mut s).unwrap();
    let mut out = std::vec![0u8; pack.len() as usize];
    assert_eq!(pack.read(media, 0, &mut out, &mut s).unwrap(), out.len());
    out
}

#[test]
fn a_written_pack_reads_back_at_any_offset() {
    let mut media = Media::new(4096);
    let mut s = sector();
    assert_eq!(status(&mut media, &mut s), Ok(None));
    assert_eq!(open(&mut media, &mut s), Err(Error::Empty));
    let data = content(70_001, 3);
    let header = install(&mut media, &data);
    assert_eq!(header.state, State::Ready);
    assert_eq!(read_all(&mut media), data);
    let pack = open(&mut media, &mut s).unwrap();
    let mut out = [0u8; 1000];
    assert_eq!(pack.read(&mut media, 69_500, &mut out, &mut s), Ok(501));
    assert_eq!(out[..501], data[69_500..]);
    assert_eq!(pack.read(&mut media, 70_001, &mut out, &mut s), Ok(0));
    assert_eq!(
        pack.read(&mut media, 70_002, &mut out, &mut s),
        Err(Error::Range)
    );
    let mut check = Check::default();
    let mut result = None;
    while result.is_none() {
        result = pack.check(&mut media, &mut check, 50, &mut s).unwrap();
    }
    assert_eq!(result, Some(true));
}

#[test]
fn writes_go_in_order_inside_the_pack_and_padding_must_be_zero() {
    let mut media = Media::new(64);
    let mut s = sector();
    let data = content(1000, 4);
    assert_eq!(
        begin(
            &mut media,
            Spec {
                length: 57 * 512,
                ..spec(&data)
            },
            &mut s
        )
        .err(),
        Some(Error::Range)
    );
    let mut writer = begin(&mut media, spec(&data), &mut s).unwrap();
    assert_eq!(
        writer.write(&mut media, 1, &chunk(&data, 1)),
        Err(Error::Range)
    );
    writer.write(&mut media, 0, &chunk(&data, 0)).unwrap();
    let mut padded = chunk(&data, 1);
    padded[511] = 1;
    assert_eq!(writer.write(&mut media, 1, &padded), Err(Error::Mismatch));
    assert_eq!(writer.commit(&mut media, &mut s), Err(Error::Range));
    writer.write(&mut media, 1, &chunk(&data, 1)).unwrap();
    assert_eq!(writer.write(&mut media, 2, &[0; 512]), Err(Error::Range));
    // A wrong CRC never becomes ready.
    let mut wrong = begin(
        &mut media,
        Spec {
            crc: 1,
            ..spec(&data)
        },
        &mut s,
    )
    .unwrap();
    wrong.write(&mut media, 0, &chunk(&data, 0)).unwrap();
    wrong.write(&mut media, 1, &chunk(&data, 1)).unwrap();
    assert_eq!(wrong.commit(&mut media, &mut s), Err(Error::Mismatch));
    assert_eq!(open(&mut media, &mut s), Err(Error::State));
    // The first writer was superseded by the second begin.
    assert_eq!(writer.commit(&mut media, &mut s), Err(Error::Stale));
}

#[test]
fn replacing_a_pack_hides_the_old_one_until_the_new_one_is_ready() {
    let mut media = Media::new(4096);
    let mut s = sector();
    let old = content(5000, 5);
    install(&mut media, &old);
    let new = content(9000, 6);
    let mut writer = begin(&mut media, spec(&new), &mut s).unwrap();
    assert_eq!(open(&mut media, &mut s), Err(Error::State));
    for index in 0..writer.header().sectors() {
        writer
            .write(&mut media, index, &chunk(&new, index))
            .unwrap();
    }
    writer.commit(&mut media, &mut s).unwrap();
    assert_eq!(read_all(&mut media), new);
}

#[test]
fn a_resumed_write_verifies_what_landed_and_continues_from_the_first_gap() {
    let mut media = Media::new(4096);
    let mut s = sector();
    let data = content(100 * 512 + 17, 8);
    let mut writer = begin(&mut media, spec(&data), &mut s).unwrap();
    for index in 0..40 {
        writer
            .write(&mut media, index, &chunk(&data, index))
            .unwrap();
    }
    // Power is lost: the host resumes in chunks of 16 sectors.
    let mut writer = Writer::resume(&mut media, &mut s).unwrap();
    assert_eq!(writer.cursor(), 0);
    let chunk_crc = |from: u64, n: u64| {
        let mut crc = Crc32::new();
        for i in from..from + n {
            crc = crc.update(&chunk(&data, i));
        }
        crc.finish()
    };
    let mut at = 0;
    loop {
        let verified = writer
            .verify(&mut media, 16, chunk_crc(at, 16), &mut s)
            .unwrap();
        if !verified.advanced {
            break;
        }
        at += 16;
    }
    assert_eq!(writer.cursor(), 32);
    for index in 32..writer.header().sectors() {
        writer
            .write(&mut media, index, &chunk(&data, index))
            .unwrap();
    }
    writer.commit(&mut media, &mut s).unwrap();
    assert_eq!(read_all(&mut media), data);
    assert_eq!(Writer::resume(&mut media, &mut s).err(), Some(Error::State));
    assert_eq!(crc(&mut media, 0, 16, &mut s), Ok(chunk_crc(0, 16)));
}

#[test]
fn power_cuts_never_expose_a_mixed_or_unverified_pack() {
    // Cut at every write of a replacement, with every tear, then finish it.
    let old = content(3 * 512 + 9, 10);
    let new = content(4 * 512 + 100, 11);
    let mut rng = Rng::new(12);
    for at in 0..8 {
        for _ in 0..3 {
            let tear = Tear::pick(&mut rng);
            let mut media = Media::new(64);
            let mut s = sector();
            install(&mut media, &old);
            media.cut = Some((at, tear));
            let finished = (|| {
                let mut writer = begin(&mut media, spec(&new), &mut s)?;
                for index in 0..writer.header().sectors() {
                    writer.write(&mut media, index, &chunk(&new, index))?;
                }
                writer.commit(&mut media, &mut s)
            })();
            media.reboot();
            match open(&mut media, &mut s) {
                Ok(_) => {
                    let now = read_all(&mut media);
                    assert!(now == old || now == new, "at {at} {tear:?}");
                    if finished.is_ok() {
                        assert_eq!(now, new);
                    }
                }
                Err(Error::State) => {
                    assert!(finished.is_err());
                    // Resume from scratch and finish.
                    let mut writer = Writer::resume(&mut media, &mut s).unwrap();
                    let header = writer.header();
                    assert_eq!(header.spec, spec(&new));
                    for index in 0..header.sectors() {
                        writer
                            .write(&mut media, index, &chunk(&new, index))
                            .unwrap();
                    }
                    writer.commit(&mut media, &mut s).unwrap();
                    assert_eq!(read_all(&mut media), new);
                }
                other => panic!("at {at} {tear:?}: {other:?}"),
            }
        }
    }
}

#[test]
fn headers_of_another_version_are_foreign() {
    let mut media = Media::new(64);
    let mut s = sector();
    install(&mut media, &content(10, 1));
    let mut header = media.get(0);
    if decode(&header).is_invalid() {
        header = media.get(1);
    }
    header[16] = 9;
    let checksum = crc32(&header[..CRC]);
    header[CRC..].copy_from_slice(&checksum.to_le_bytes());
    media.set(0, header);
    assert_eq!(status(&mut media, &mut s), Err(Error::Foreign));
}

impl Replica {
    fn is_invalid(&self) -> bool {
        matches!(self, Replica::Invalid)
    }
}
