//! Ordinary commands over the owned region's volumes: the table, the terrain
//! pack and the file store. Bulk data moves through the binary transfer
//! session (`crate::transfer`) that `VOLUME TRANSFER` enters.

use core::fmt::Write;
use device_api::bulk::{Error as MediaError, ReadWrite};
use firmware_services::volume::{self, Layout};
use firmware_services::{files, pack};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    /// Write the default table over a blank or legacy region.
    Init,
    /// Enter the binary transfer session; the composition owns the switch.
    Transfer,
    Terrain,
    Files,
    FilesFormat,
    /// List files from this slot on.
    FilesList(u8),
    FilesStat(files::Name),
    FilesDelete(files::Name),
}

pub fn parse(name: &str, words: &mut core::str::SplitAsciiWhitespace<'_>) -> Option<Command> {
    let command = match (name, words.next()) {
        ("VOLUME", None | Some("STATUS")) => Command::Status,
        ("VOLUME", Some("INIT")) => Command::Init,
        ("VOLUME", Some("TRANSFER")) => Command::Transfer,
        ("TERRAIN", None | Some("STATUS")) => Command::Terrain,
        ("FILES", None | Some("STATUS")) => Command::Files,
        ("FILES", Some("FORMAT")) => match words.next() {
            Some("CONFIRM") => Command::FilesFormat,
            _ => return None,
        },
        ("FILES", Some("LIST")) => match words.next() {
            None => Command::FilesList(0),
            Some(slot) => {
                let slot: u8 = slot.parse().ok()?;
                (usize::from(slot) < files::CAPACITY).then_some(Command::FilesList(slot))?
            }
        },
        ("FILES", Some(verb @ ("STAT" | "DELETE"))) => {
            let name = files::Name::new(words.next()?).ok()?;
            if verb == "STAT" {
                Command::FilesStat(name)
            } else {
                Command::FilesDelete(name)
            }
        }
        _ => return None,
    };
    words.next().is_none().then_some(command)
}

pub fn media_status(error: MediaError) -> &'static str {
    match error {
        MediaError::Unsupported => "UNSUPPORTED",
        MediaError::Unavailable => "UNAVAILABLE",
        MediaError::Range => "INVALID",
        MediaError::Failed => "FAILED",
    }
}

pub fn volume_status(error: volume::Error) -> &'static str {
    match error {
        volume::Error::Media(e) => media_status(e),
        volume::Error::Foreign => "FOREIGN",
        volume::Error::Missing => "UNINITIALIZED",
        volume::Error::Small => "UNSUPPORTED",
    }
}

pub fn pack_status(error: pack::Error) -> &'static str {
    match error {
        pack::Error::Media(e) => media_status(e),
        pack::Error::Foreign => "FOREIGN",
        pack::Error::Empty => "EMPTY",
        pack::Error::State => "STATE",
        pack::Error::Range => "INVALID",
        pack::Error::Mismatch => "MISMATCH",
        pack::Error::Stale => "STALE",
    }
}

pub fn files_status(error: files::Error) -> &'static str {
    match error {
        files::Error::Media(e) => media_status(e),
        files::Error::Uninitialized => "UNINITIALIZED",
        files::Error::Foreign => "FOREIGN",
        files::Error::Corrupt => "CORRUPT",
        files::Error::Name => "INVALID",
        files::Error::TooLarge => "TOO_LARGE",
        files::Error::Full => "FULL",
        files::Error::NotFound => "NOT_FOUND",
        files::Error::Stale => "STALE",
        files::Error::Range => "INVALID",
        files::Error::Mismatch => "MISMATCH",
    }
}

fn hex(out: &mut impl Write, bytes: &[u8]) {
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
}

/// The tag up to its padding, if it is printable.
pub fn tag_text(tag: &[u8; 16]) -> &str {
    let end = tag.iter().position(|b| *b == 0).unwrap_or(tag.len());
    match core::str::from_utf8(&tag[..end]) {
        Ok(text) if text.bytes().all(|b| b.is_ascii_graphic()) => text,
        _ => "?",
    }
}

fn entry(out: &mut impl Write, entry: &files::Entry) {
    let _ = write!(
        out,
        "{}:{}:{:08x}:{}",
        entry.name.as_str(),
        entry.length,
        entry.crc,
        entry.revision
    );
}

/// `Transfer` is the composition's to run; it answers `ACCEPTED` here only
/// after checking the region can be reached.
pub fn execute(command: Command, media: &mut impl ReadWrite, out: &mut impl Write) -> &'static str {
    let mut sector = [0u8; 512];
    match command {
        Command::Status | Command::Init => {
            let found = if command == Command::Init {
                volume::create(media, &mut sector).map(Layout::Table)
            } else {
                volume::read(media, &mut sector)
            };
            let region = match media.owned_info() {
                Ok(info) => info.sectors,
                Err(e) => return media_status(e),
            };
            match found {
                Ok(layout) => {
                    let _ = write!(out, "layout={} sectors={}", layout.name(), region);
                    if let Layout::Table(table) = layout {
                        let _ = write!(
                            out,
                            " generation={} unallocated={}",
                            table.generation,
                            table.unallocated(region)
                        );
                        for v in table.volumes() {
                            let _ = write!(
                                out,
                                " {}={}+{}:{}",
                                v.label().to_ascii_lowercase_str(),
                                v.start,
                                v.sectors,
                                v.kind.name()
                            );
                        }
                    }
                    "OK"
                }
                Err(volume::Error::Foreign) => {
                    let _ = write!(out, "layout=foreign sectors={region}");
                    "FOREIGN"
                }
                Err(e) => volume_status(e),
            }
        }
        Command::Transfer => match volume::read(media, &mut sector) {
            Ok(_) => "ACCEPTED",
            Err(e) => volume_status(e),
        },
        Command::Terrain => {
            let mut vol = match volume::open(media, &volume::TERRAIN, &mut sector) {
                Ok(v) => v,
                Err(e) => return volume_status(e),
            };
            let sectors = vol.extent().sectors;
            match pack::status(&mut vol, &mut sector) {
                Ok(None) => {
                    let _ = write!(out, "state=empty capacity={}", pack::capacity(sectors));
                    "OK"
                }
                Ok(Some(h)) => {
                    let _ = write!(
                        out,
                        "state={} generation={} length={} crc32={:08x} tag={} stamp={} capacity={} digest=",
                        h.state.name(),
                        h.generation,
                        h.spec.length,
                        h.spec.crc,
                        tag_text(&h.spec.tag),
                        h.spec.stamp,
                        pack::capacity(sectors)
                    );
                    hex(out, &h.spec.digest);
                    "OK"
                }
                Err(e) => pack_status(e),
            }
        }
        Command::Files | Command::FilesFormat => {
            let mut vol = match volume::open(media, &volume::FILES, &mut sector) {
                Ok(v) => v,
                Err(e) => return volume_status(e),
            };
            let dir = if command == Command::FilesFormat {
                files::format(&mut vol, 0, &mut sector)
            } else {
                files::mount(&mut vol, &mut sector)
            };
            match dir {
                Ok(dir) => {
                    let _ = write!(
                        out,
                        "state=ready files={} capacity={} max_file={} generation={} store={:016x} data_sectors={}",
                        dir.len(),
                        files::CAPACITY,
                        files::MAX_FILE,
                        dir.generation(),
                        dir.store_id(),
                        dir.data_sectors()
                    );
                    "OK"
                }
                Err(files::Error::Uninitialized) => {
                    let _ = write!(out, "state=uninitialized");
                    "OK"
                }
                Err(e) => files_status(e),
            }
        }
        Command::FilesList(start) => {
            let mut vol = match volume::open(media, &volume::FILES, &mut sector) {
                Ok(v) => v,
                Err(e) => return volume_status(e),
            };
            let dir = match files::mount(&mut vol, &mut sector) {
                Ok(dir) => dir,
                Err(e) => return files_status(e),
            };
            // Bounded reply: up to eight files, then where to continue.
            let mut shown = 0;
            let mut next = None;
            for slot in usize::from(start)..files::CAPACITY {
                match dir.entry(&mut vol, slot, &mut sector) {
                    Ok(Some(e)) => {
                        if shown == 8 {
                            next = Some(slot);
                            break;
                        }
                        let _ = write!(out, "{}", if shown == 0 { "" } else { " " });
                        entry(out, &e);
                        shown += 1;
                    }
                    Ok(None) => {}
                    Err(e) => return files_status(e),
                }
            }
            if let Some(next) = next {
                let _ = write!(out, " next={next}");
            }
            "OK"
        }
        Command::FilesStat(name) | Command::FilesDelete(name) => {
            let mut vol = match volume::open(media, &volume::FILES, &mut sector) {
                Ok(v) => v,
                Err(e) => return volume_status(e),
            };
            let result = files::mount(&mut vol, &mut sector).and_then(|dir| {
                if let Command::FilesDelete(_) = command {
                    dir.delete(&mut vol, &name, &mut sector).map(|_| None)
                } else {
                    dir.find(&mut vol, &name, &mut sector)?
                        .ok_or(files::Error::NotFound)
                        .map(Some)
                }
            });
            match result {
                Ok(Some(e)) => {
                    entry(out, &e);
                    "OK"
                }
                Ok(None) => "OK",
                Err(e) => files_status(e),
            }
        }
    }
}

trait Lower {
    fn to_ascii_lowercase_str(&self) -> Lowercase<'_>;
}

impl Lower for str {
    fn to_ascii_lowercase_str(&self) -> Lowercase<'_> {
        Lowercase(self)
    }
}

struct Lowercase<'a>(&'a str);

impl core::fmt::Display for Lowercase<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for c in self.0.chars() {
            f.write_char(c.to_ascii_lowercase())?;
        }
        Ok(())
    }
}
