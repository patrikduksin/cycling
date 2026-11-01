//! The media a device trace started on, rebuilt from its header and what a
//! host pulled from the device after the session (`cycling trace pull`).
//! The header names each medium as VANA found it at power-on; the pulled
//! copies are later. The ride journals only grow, so they are rewound to
//! the ends the header recorded. A file is the one of power-on when FILES is
//! the same store and the file still has the revision the header recorded:
//! the revision names the directory commit that wrote it. Its CRC cannot,
//! since a file that ends in the CRC32 of its bytes, as workouts, the bests
//! and each slot of the ride facts table do, has a whole-file CRC32 that
//! depends only on its length. The ride facts table VANA rewrote since is
//! taken back to the rides that had ended at power-on and checked against
//! the digest of its slots the header recorded. What the pull cannot give
//! back is a [`Gap`], and its medium starts without it.
use crate::bulk::{Presence, SimBulk};
use crate::replay::Origin;
use crate::storage::FileFlash;
use device_api::bulk::SECTOR_SIZE;
use firmware_services::trace::origin::{self, Config, FileEntry, RideLog};
use firmware_services::{checksum::crc32, files, pack, sector_log, volume};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::Path;
use vana::ride::bulk::{CHECKPOINT_RECORDS, TAG, encode_catalog};
use vana::ride::facts;
use vana::ride::log::{Catalog, Kind, SLOT_SIZE, Scanner, Slot, decode};

/// What a host pulled from the device after the session, through the
/// read-only paths: ride export and `files get`.
#[derive(Clone, Debug, Default)]
pub struct Pulled {
    /// The internal ride journal's slots from slot 0, as `EXPORT SLOT`
    /// reads them.
    pub internal: Option<Vec<u8>>,
    pub rides: Option<Rides>,
    pub files: Option<Files>,
    /// The local terrain pack whose SHA-256 is the header's digest.
    pub terrain: Option<Vec<u8>>,
}

/// The RIDES log as `EXPORT BULK` reads it.
#[derive(Clone, Debug, Default)]
pub struct Rides {
    pub store: u64,
    pub generation: u64,
    /// Its records' slots from record 0.
    pub slots: Vec<u8>,
    /// Records whose sector container was invalid; their slots are zeros.
    pub invalid: Vec<u64>,
}

/// FILES as `files get` reads it: the store's id, and each file the header
/// lists that is still there, by name, with its revision.
#[derive(Clone, Debug, Default)]
pub struct Files {
    pub store: u64,
    pub files: BTreeMap<String, (u32, Vec<u8>)>,
    /// Files the header lists whose read failed in the pull.
    pub unread: BTreeSet<String>,
}

/// Something the rebuilt media holds otherwise than the device did at
/// power-on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gap {
    /// The internal journal holds fewer slots than at power-on, so it was
    /// cleared since, or it was not pulled; the rebuilt journal is empty.
    Journal { slots: u32, pulled: Option<usize> },
    /// The internal journal holds another slot where VANA's last one was at
    /// power-on, so it was cleared and filled again since; the rebuilt
    /// journal is empty.
    JournalReplaced { slots: u32 },
    /// RIDES was not pulled; the rebuilt card has no ride log.
    RidesNotPulled,
    /// RIDES now holds another log, so it was cleared or initialized since;
    /// the rebuilt card has no ride log.
    RidesReplaced { store: u64, now: u64 },
    /// RIDES holds fewer records than at power-on; the rebuilt card has no
    /// ride log.
    RidesShort { head: u64, pulled: u64 },
    /// Records whose sector container was invalid on the device; the rebuilt
    /// log holds valid records of zeros there.
    RidesInvalid(Vec<u64>),
    /// VANA could not prove where the card's ride log ended at power-on and
    /// kept it read only; the rebuilt log holds its `head` records and takes
    /// appends.
    RidesUncertain { head: u64 },
    /// VANA could not use the card's ride log at power-on; the rebuilt card
    /// has no ride log.
    RidesUnusable,
    /// FILES was not pulled, or it now holds another store, so it was
    /// formatted since: no file of power-on is recovered.
    Files { store: u64, now: Option<u64> },
    /// A file that is gone from the device (`now` none) or was written
    /// again since power-on, now at revision `now`; its bytes at power-on
    /// are lost and the rebuilt FILES leaves it out.
    File {
        name: String,
        revision: u32,
        now: Option<u32>,
    },
    /// A file whose read failed in the pull; the rebuilt FILES leaves it out.
    FileUnread { name: String, revision: u32 },
    /// No local pack has the SHA-256 the header names; the rebuilt card has
    /// no terrain pack.
    Terrain { tag: String, digest: [u8; 32] },
}

impl fmt::Display for Gap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Journal { slots, pulled } => write!(
                f,
                "internal ride journal: VANA found {slots} slots at power-on, {}; it starts empty",
                match pulled {
                    Some(pulled) =>
                        format!("the device holds {pulled} now, so it was cleared since"),
                    None => "and it was not pulled".into(),
                }
            ),
            Self::JournalReplaced { slots } => write!(
                f,
                "internal ride journal: slot {} on the device is not the one VANA found at power-on, so the journal was cleared since; it starts empty",
                slots - 1
            ),
            Self::RidesNotPulled => {
                write!(f, "RIDES: not pulled; the card starts without a ride log")
            }
            Self::RidesReplaced { store, now } => write!(
                f,
                "RIDES: the device holds log {now:016x} now, not {store:016x} of power-on, so it was cleared or initialized since; the card starts without a ride log"
            ),
            Self::RidesShort { head, pulled } => write!(
                f,
                "RIDES: VANA found {head} records at power-on, the pull holds {pulled}; the card starts without a ride log"
            ),
            Self::RidesInvalid(records) => write!(
                f,
                "RIDES: the containers of records {} were invalid on the device; they start as valid records of zeros",
                records
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::RidesUncertain { head } => write!(
                f,
                "RIDES: VANA could not prove where the ride log ended at power-on and kept it read only; the card starts with its {head} records as a log that takes rides"
            ),
            Self::RidesUnusable => write!(
                f,
                "RIDES: VANA could not use the ride log at power-on: the volume table or the log's metadata was corrupt, lost or foreign, or the card did not read; the card starts without a ride log"
            ),
            Self::Files { store, now: None } => {
                write!(f, "FILES (store {store:016x}): not pulled; it starts empty")
            }
            Self::Files {
                store,
                now: Some(now),
            } => write!(
                f,
                "FILES: the device holds store {now:016x} now, not {store:016x} of power-on, so it was formatted since and no file of power-on can be recovered; it starts empty"
            ),
            Self::File {
                name,
                revision,
                now: None,
            } => write!(
                f,
                "file {name} (revision {revision}): gone from the device, so its bytes at power-on cannot be recovered; FILES starts without it"
            ),
            Self::File {
                name,
                revision,
                now: Some(now),
            } => write!(
                f,
                "file {name}: written again since power-on (revision {now}, was {revision}), so its bytes at power-on cannot be recovered; FILES starts without it"
            ),
            Self::FileUnread { name, revision } => write!(
                f,
                "file {name} (revision {revision}): its read failed in the pull; FILES starts without it"
            ),
            Self::Terrain { tag, digest } => write!(
                f,
                "terrain pack {tag} (SHA-256 {}): no local pack under .local/ has it; the card starts without one",
                hex(digest)
            ),
        }
    }
}

/// The media the trace `trace` started on, from its header and `pulled`,
/// and what they could not give back.
pub fn rebuild(trace: &[u8], pulled: &Pulled) -> Result<(Origin, Vec<Gap>), String> {
    let (header, _) = firmware_services::trace::decode(trace)
        .map_err(|error| format!("the trace does not decode: {error:?}"))?;
    let config = Config::decode(header.config).ok_or("the header's settings do not decode")?;
    let media = media_of(&header)?;
    let boot = header.boot.max(1);
    let mut gaps = Vec::new();
    let internal = media
        .journal
        .and_then(|journal| internal_at_power_on(journal, pulled, &mut gaps));
    let rides = match media.rides {
        Some(RideLog::Log { store, head }) => rides_at_power_on(store, head, pulled, &mut gaps),
        Some(RideLog::Uncertain { store, head }) => {
            gaps.push(Gap::RidesUncertain { head });
            rides_at_power_on(store, head, pulled, &mut gaps)
        }
        Some(RideLog::Unusable) => {
            gaps.push(Gap::RidesUnusable);
            None
        }
        Some(RideLog::Uninitialized) | None => None,
    };
    let ended = ended(internal.unwrap_or_default(), rides.map_or(&[], |log| log.1));
    let flash = internal_flash(boot, config.settings, internal)?;
    let bulk = SimBulk::default();
    bulk.set_presence(Presence::Ready);
    let mut card = bulk.host();
    let mut sector = [0u8; SECTOR_SIZE];
    volume::create(&mut card, &mut sector).map_err(|e| format!("volume table: {e:?}"))?;
    let mut out = String::new();
    let status = firmware_console::commands::volumes::execute(
        firmware_console::commands::volumes::Command::AddStore,
        &mut card,
        &mut out,
    );
    if status != "OK" {
        return Err(format!("STORE: {status} {out}"));
    }
    if let Some((log, slots)) = rides {
        write_rides(&mut card, log, slots)?;
    }
    if let Some(listed) = media.files {
        write_files(&mut card, &listed, media.facts, pulled, &ended, &mut gaps)?;
    }
    if let Some(terrain) = media.terrain {
        write_terrain(&mut card, &terrain, pulled, &mut gaps)?;
    }
    Ok((Origin { flash, bulk, boot }, gaps))
}

/// What a report of `gaps` says: one line each, or that nothing is missing.
pub fn report(gaps: &[Gap]) -> String {
    if gaps.is_empty() {
        return "every medium the header names was rebuilt\n".into();
    }
    gaps.iter().map(|gap| format!("{gap}\n")).collect()
}

/// The media the header of `trace` names, one line each, as the pull tool
/// reads them: what to copy from the device and where each log ended.
pub fn media(trace: &[u8]) -> Result<String, String> {
    let (header, _) = firmware_services::trace::decode(trace)
        .map_err(|error| format!("the trace does not decode: {error:?}"))?;
    let media = media_of(&header)?;
    let mut out = format!("boot {}\n", header.boot);
    match media.journal {
        Some(journal) => out += &format!("journal {}\n", journal.next),
        None => out += "journal none\n",
    }
    match media.rides {
        Some(RideLog::Log { store, head }) => out += &format!("rides log {store:016x} {head}\n"),
        Some(RideLog::Uncertain { store, head }) => {
            out += &format!("rides uncertain {store:016x} {head}\n")
        }
        Some(RideLog::Uninitialized) => out += "rides uninitialized\n",
        Some(RideLog::Unusable) => out += "rides unusable\n",
        None => out += "rides none\n",
    }
    match media.files {
        Some(files) => {
            out += &format!("files {:016x} {}\n", files.store, files.generation);
            for entry in files.entries() {
                out += &format!(
                    "file {} {} {:08x} {}\n",
                    entry.name, entry.length, entry.crc, entry.revision
                );
            }
        }
        None => out += "files none\n",
    }
    match media.terrain {
        Some(terrain) => {
            out += &format!(
                "terrain {} {} {} {}\n",
                terrain.length,
                hex(&terrain.digest),
                if terrain.ready { "ready" } else { "writing" },
                tag(&terrain.tag)
            )
        }
        None => out += "terrain none\n",
    }
    Ok(out)
}

/// The internal journal's slots as VANA found them, when the pull still
/// holds them: as many, the last of them the one the header names.
fn internal_at_power_on<'a>(
    journal: origin::Journal,
    pulled: &'a Pulled,
    gaps: &mut Vec<Gap>,
) -> Option<&'a [u8]> {
    let slots = journal.next;
    let wanted = slots as usize * SLOT_SIZE;
    let Some(now) = pulled.internal.as_ref().filter(|now| now.len() >= wanted) else {
        gaps.push(Gap::Journal {
            slots,
            pulled: pulled.internal.as_ref().map(|now| now.len() / SLOT_SIZE),
        });
        return None;
    };
    let kept = &now[..wanted];
    let last = kept
        .as_chunks::<SLOT_SIZE>()
        .0
        .last()
        .map(|slot| crc32(slot));
    if last != journal.last {
        gaps.push(Gap::JournalReplaced { slots });
        return None;
    }
    Some(kept)
}

/// The RIDES log of epoch `store` and its first `head` records' slots.
fn rides_at_power_on<'a>(
    store: u64,
    head: u64,
    pulled: &'a Pulled,
    gaps: &mut Vec<Gap>,
) -> Option<(&'a Rides, &'a [u8])> {
    let Some(log) = &pulled.rides else {
        gaps.push(Gap::RidesNotPulled);
        return None;
    };
    if log.store != store {
        gaps.push(Gap::RidesReplaced {
            store,
            now: log.store,
        });
        return None;
    }
    let records = (log.slots.len() / SLOT_SIZE) as u64;
    if records < head {
        gaps.push(Gap::RidesShort {
            head,
            pulled: records,
        });
        return None;
    }
    let invalid: Vec<u64> = log.invalid.iter().copied().filter(|r| *r < head).collect();
    if !invalid.is_empty() {
        gaps.push(Gap::RidesInvalid(invalid));
    }
    Some((log, &log.slots[..head as usize * SLOT_SIZE]))
}

/// The rides whose last record precedes power-on, as the ride facts table
/// names them: journal `i` or `b`, and ride id.
fn ended(internal: &[u8], rides: &[u8]) -> BTreeSet<(u8, u32)> {
    let mut ended = BTreeSet::new();
    for (journal, slots) in [(b'i', internal), (b'b', rides)] {
        for slot in slots.as_chunks::<SLOT_SIZE>().0 {
            if let Some(ride) = ends(slot) {
                ended.insert((journal, ride));
            }
        }
    }
    ended
}

/// The ride whose last record `slot` is, if it is one.
fn ends(slot: &[u8; SLOT_SIZE]) -> Option<u32> {
    decode(&Slot(*slot))
        .filter(|entry| matches!(entry.kind, Kind::Finish | Kind::Recovered | Kind::Full))
        .map(|entry| entry.ride_id)
}

/// The shell's settings record and the internal journal's slots.
fn internal_flash(boot: u32, settings: &[u8], journal: Option<&[u8]>) -> Result<FileFlash, String> {
    let blank = FileFlash::memory();
    let (settings, _) = firmware_shell::preferences::decode(Some(settings));
    firmware_shell::storage::Store::new(blank.clone())
        .save(settings)
        .map_err(|error| format!("the settings do not save: {error:?}"))?;
    let (_, configuration, mut data) = blank.image().expect("memory media");
    let journal = journal.unwrap_or_default();
    data.get_mut(..journal.len())
        .ok_or("the internal journal is larger than its region")?
        .copy_from_slice(journal);
    Ok(FileFlash::from_image(boot - 1, configuration, data))
}

/// The log of `log`'s epoch holding `slots`, checkpointed as VANA does
/// when nothing interrupts it: after each ride's last record and every
/// `CHECKPOINT_RECORDS` records. After a power cut the device's
/// checkpoints can sit elsewhere, so a boot here replays other records to
/// the same rides.
fn write_rides(card: &mut SimBulk, log: &Rides, slots: &[u8]) -> Result<(), String> {
    let mut sector = [0u8; SECTOR_SIZE];
    let mut volume =
        volume::open(card, &volume::RIDES, &mut sector).map_err(|e| format!("RIDES: {e:?}"))?;
    let initial = encode_catalog(&Catalog {
        next_ride_id: 1,
        ..Catalog::default()
    });
    let mut epoch = sector_log::restore(
        &mut volume,
        TAG,
        log.store,
        log.generation,
        &initial,
        &mut sector,
    )
    .map_err(|e| format!("RIDES: {e:?}"))?;
    let mut scanner = Scanner::default();
    let mut since = 0;
    for (index, slot) in slots.as_chunks::<SLOT_SIZE>().0.iter().enumerate() {
        epoch
            .append(&mut volume, slot, &mut sector)
            .map_err(|e| format!("RIDES record {index}: {e:?}"))?;
        scanner.accept_slot(index, slot);
        since += 1;
        if ends(slot).is_none() && since < CHECKPOINT_RECORDS {
            continue;
        }
        epoch
            .checkpoint(
                &mut volume,
                &encode_catalog(&scanner.catalog()),
                &mut sector,
            )
            .map_err(|e| format!("RIDES checkpoint at {index}: {e:?}"))?;
        since = 0;
    }
    Ok(())
}

/// FILES as the header lists it: each file still at its revision, the ride
/// facts table taken back to the rides that had ended when that gives the
/// table of digest `facts`, and nothing else.
fn write_files(
    card: &mut SimBulk,
    listed: &origin::Files<'_>,
    facts: Option<u32>,
    pulled: &Pulled,
    ended: &BTreeSet<(u8, u32)>,
    gaps: &mut Vec<Gap>,
) -> Result<(), String> {
    let mut kept: Vec<(files::Name, u32, Vec<u8>)> = Vec::new();
    match &pulled.files {
        Some(now) if now.store == listed.store => {
            for entry in listed.entries() {
                if now.unread.contains(entry.name) {
                    gaps.push(Gap::FileUnread {
                        name: entry.name.into(),
                        revision: entry.revision,
                    });
                    continue;
                }
                let pulled = now.files.get(entry.name);
                let bytes = match pulled {
                    Some((revision, bytes)) if *revision == entry.revision => Some(bytes.clone()),
                    Some((_, bytes)) if entry.name == facts::TABLE => {
                        facts_at_power_on(bytes, ended)
                            .filter(|table| facts.is_some() && facts::table_digest(table) == facts)
                    }
                    _ => None,
                };
                match bytes {
                    Some(bytes) if same(&entry, &bytes) => {
                        let name = files::Name::new(entry.name).map_err(|e| format!("{e:?}"))?;
                        kept.push((name, entry.revision, bytes));
                    }
                    _ => gaps.push(Gap::File {
                        name: entry.name.into(),
                        revision: entry.revision,
                        now: pulled.map(|(revision, _)| *revision),
                    }),
                }
            }
        }
        now => gaps.push(Gap::Files {
            store: listed.store,
            now: now.as_ref().map(|now| now.store),
        }),
    }
    let kept: Vec<(files::Name, u32, &[u8])> = kept
        .iter()
        .map(|(name, revision, bytes)| (*name, *revision, bytes.as_slice()))
        .collect();
    let mut sector = [0u8; SECTOR_SIZE];
    let mut volume =
        volume::open(card, &volume::FILES, &mut sector).map_err(|e| format!("FILES: {e:?}"))?;
    files::restore(
        &mut volume,
        listed.store,
        listed.generation,
        &kept,
        &mut sector,
    )
    .map_err(|e| format!("FILES: {e:?}"))?;
    Ok(())
}

/// The ride facts table `now` less the records of rides that had not ended
/// at power-on: VANA appends a ride's record when it is saved.
fn facts_at_power_on(now: &[u8], ended: &BTreeSet<(u8, u32)>) -> Option<Vec<u8>> {
    let stride = facts::table_stride(now)?;
    let mut slots = now.chunks_exact(stride);
    let mut table = slots.next()?.to_vec();
    for slot in slots {
        if facts::decode_slot(slot)
            .is_none_or(|record| ended.contains(&(record.journal, record.ride_id)))
        {
            table.extend_from_slice(slot);
        }
    }
    Some(table)
}

fn same(entry: &FileEntry<'_>, bytes: &[u8]) -> bool {
    bytes.len() == entry.length as usize && crc32(bytes) == entry.crc
}

fn write_terrain(
    card: &mut SimBulk,
    terrain: &origin::Terrain,
    pulled: &Pulled,
    gaps: &mut Vec<Gap>,
) -> Result<(), String> {
    let Some(bytes) = pulled.terrain.as_ref().filter(|bytes| {
        bytes.len() as u64 == terrain.length && Sha256::digest(bytes)[..] == terrain.digest
    }) else {
        gaps.push(Gap::Terrain {
            tag: tag(&terrain.tag),
            digest: terrain.digest,
        });
        return Ok(());
    };
    let header = pack::Header {
        state: if terrain.ready {
            pack::State::Ready
        } else {
            pack::State::Writing
        },
        generation: terrain.generation,
        spec: pack::Spec {
            length: terrain.length,
            crc: crc32(bytes),
            tag: terrain.tag,
            digest: terrain.digest,
            stamp: 0,
        },
    };
    let mut sector = [0u8; SECTOR_SIZE];
    let mut volume =
        volume::open(card, &volume::TERRAIN, &mut sector).map_err(|e| format!("TERRAIN: {e:?}"))?;
    pack::restore(&mut volume, header, bytes, &mut sector).map_err(|e| format!("TERRAIN: {e:?}"))
}

impl Pulled {
    /// What `cycling trace pull` wrote into `dir`: `internal.slots`;
    /// `rides.slots` with `rides.epoch` (`store <hex>`, `generation <n>` and
    /// `invalid <records>` lines); `files.list` (a `store <hex>` line, then
    /// `file <revision> <name>` for each file under `files/` by its name and
    /// `unread <name>` for each whose read failed); and `terrain.pack`. A
    /// medium without its file was not pulled.
    pub fn read(dir: &Path) -> io::Result<Self> {
        let optional = |name: &str| match std::fs::read(dir.join(name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        };
        let rides = match (optional("rides.epoch")?, optional("rides.slots")?) {
            (Some(epoch), Some(slots)) => Some(rides_of(&String::from_utf8_lossy(&epoch), slots)?),
            _ => None,
        };
        let files = optional("files.list")?
            .map(|list| files_of(&String::from_utf8_lossy(&list), &dir.join("files")))
            .transpose()?;
        Ok(Self {
            internal: optional("internal.slots")?,
            rides,
            files,
            terrain: optional("terrain.pack")?,
        })
    }
}

fn rides_of(text: &str, slots: Vec<u8>) -> io::Result<Rides> {
    let malformed = || io::Error::other(format!("rides.epoch is malformed: {text:?}"));
    let mut rides = Rides {
        slots,
        ..Rides::default()
    };
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[..] {
            ["store", store] => {
                rides.store = u64::from_str_radix(store, 16).map_err(|_| malformed())?;
            }
            ["generation", generation] => {
                rides.generation = generation.parse().map_err(|_| malformed())?;
            }
            ["invalid", ref records @ ..] => {
                for record in records {
                    rides.invalid.push(record.parse().map_err(|_| malformed())?);
                }
            }
            [] => {}
            _ => return Err(malformed()),
        }
    }
    if rides.store == 0 || rides.generation == 0 {
        return Err(malformed());
    }
    Ok(rides)
}

fn files_of(text: &str, dir: &Path) -> io::Result<Files> {
    let malformed = || io::Error::other(format!("files.list is malformed: {text:?}"));
    let mut files = Files::default();
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[..] {
            ["store", store] => {
                files.store = u64::from_str_radix(store, 16).map_err(|_| malformed())?;
            }
            ["file", revision, name] => {
                let name = file_name(name).ok_or_else(malformed)?;
                let revision = revision.parse().map_err(|_| malformed())?;
                let bytes = std::fs::read(dir.join(name))?;
                files.files.insert(name.into(), (revision, bytes));
            }
            ["unread", name] => {
                files
                    .unread
                    .insert(file_name(name).ok_or_else(malformed)?.into());
            }
            [] => {}
            _ => return Err(malformed()),
        }
    }
    if files.store == 0 {
        return Err(malformed());
    }
    Ok(files)
}

/// `name` when it is a FILES name that stays under `files/`.
fn file_name(name: &str) -> Option<&str> {
    (files::Name::new(name).is_ok() && !name.split('/').any(|part| part == "..")).then_some(name)
}

fn media_of<'a>(
    header: &firmware_services::trace::Header<'a>,
) -> Result<origin::Media<'a>, String> {
    if header.media.is_empty() {
        return Err(
            "the header names no media: the build that recorded it did not write them".into(),
        );
    }
    origin::Media::decode(header.media).ok_or_else(|| "the header's media do not decode".into())
}

fn tag(tag: &[u8; 16]) -> String {
    String::from_utf8_lossy(tag).trim_end_matches('\0').into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
