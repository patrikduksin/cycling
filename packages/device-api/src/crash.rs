//! Small, versioned panic record format for reset-persistent device storage.

pub const WORDS: usize = 7;
const MAGIC: u32 = 0x4352_5348;
const SCHEMA: u32 = 1;
pub const COMMITTED: u32 = 0xc06c_a55a;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Reset {
    #[default]
    Unknown,
    Power,
    Software,
    Watchdog,
    Brownout,
    Usb,
    Other,
}

impl Reset {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Power => "power",
            Self::Software => "software",
            Self::Watchdog => "watchdog",
            Self::Brownout => "brownout",
            Self::Usb => "usb",
            Self::Other => "other",
        }
    }

    pub const fn short(self) -> &'static [u8] {
        match self {
            Self::Unknown => b"UNK",
            Self::Power => b"PWR",
            Self::Software => b"SW",
            Self::Watchdog => b"WDT",
            Self::Brownout => b"BROWN",
            Self::Usb => b"USB",
            Self::Other => b"OTHER",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum Kind {
    #[default]
    Panic = 1,
    Controlled = 2,
}

impl Kind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::Controlled => "controlled",
        }
    }

    pub const fn short(self) -> &'static [u8] {
        match self {
            Self::Panic => b"PANIC",
            Self::Controlled => b"TEST",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Report {
    pub kind: Kind,
    version: [u8; 8],
    version_len: u8,
}

impl Report {
    pub fn version(&self) -> &str {
        core::str::from_utf8(&self.version[..usize::from(self.version_len)]).unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Marker {
    #[default]
    None,
    Valid(Report),
    Invalid,
}

impl Marker {
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Valid(report) => report.kind.name(),
            Self::Invalid => "invalid",
        }
    }

    pub const fn short(self) -> &'static [u8] {
        match self {
            Self::None => b"NONE",
            Self::Valid(report) => report.kind.short(),
            Self::Invalid => b"BAD",
        }
    }

    pub fn firmware(&self) -> &str {
        match self {
            Self::Valid(report) => report.version(),
            _ => "",
        }
    }
}

pub fn encode(kind: Kind, version: &str) -> [u32; WORDS] {
    let mut bytes = [0u8; 8];
    let length = version.len().min(bytes.len());
    bytes[..length].copy_from_slice(&version.as_bytes()[..length]);
    let mut words = [
        MAGIC,
        SCHEMA | ((kind as u32) << 8) | ((length as u32) << 16),
        u32::from_le_bytes(bytes[..4].try_into().unwrap()),
        u32::from_le_bytes(bytes[4..].try_into().unwrap()),
        0,
        0,
        0,
    ];
    words[4] = checksum(&words[..4]);
    words[6] = COMMITTED;
    words
}

pub fn inspect(words: &[u32; WORDS]) -> Marker {
    if words[6] == 0 && words.iter().all(|word| *word == 0) {
        return Marker::None;
    }
    if words[6] != COMMITTED || words[0] != MAGIC || words[1] & 0xff != SCHEMA || words[5] != 0 {
        return Marker::Invalid;
    }
    if checksum(&words[..4]) != words[4] {
        return Marker::Invalid;
    }
    let kind = match (words[1] >> 8) & 0xff {
        1 => Kind::Panic,
        2 => Kind::Controlled,
        _ => return Marker::Invalid,
    };
    let length = ((words[1] >> 16) & 0xff) as usize;
    if length > 8 {
        return Marker::Invalid;
    }
    let mut version = [0; 8];
    version[..4].copy_from_slice(&words[2].to_le_bytes());
    version[4..].copy_from_slice(&words[3].to_le_bytes());
    let report = Report {
        kind,
        version,
        version_len: length as u8,
    };
    if report.version().is_empty() {
        Marker::Invalid
    } else {
        Marker::Valid(report)
    }
}

fn checksum(words: &[u32]) -> u32 {
    words.iter().fold(0x811c_9dc5, |hash, word| {
        word.to_le_bytes().iter().fold(hash, |hash, byte| {
            (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_record_round_trips_bounded_version() {
        let record = encode(Kind::Controlled, "123456789");
        let Marker::Valid(report) = inspect(&record) else {
            panic!("valid record rejected")
        };
        assert_eq!(report.kind, Kind::Controlled);
        assert_eq!(report.version(), "12345678");
    }

    #[test]
    fn absent_torn_corrupt_and_outdated_records_are_safe() {
        assert_eq!(inspect(&[0; WORDS]), Marker::None);
        let valid = encode(Kind::Panic, "0.1.0");
        for index in 0..WORDS {
            let mut damaged = valid;
            damaged[index] ^= 1;
            assert_eq!(inspect(&damaged), Marker::Invalid, "word {index}");
        }
        let mut torn = valid;
        torn[6] = 0;
        assert_eq!(inspect(&torn), Marker::Invalid);
        let mut outdated = valid;
        outdated[1] = (outdated[1] & !0xff) | 2;
        assert_eq!(inspect(&outdated), Marker::Invalid);
    }
}
