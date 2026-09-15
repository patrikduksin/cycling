//! Position fixes and receiver observations.
pub const STALE_MS: u64 = 3_000;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixState {
    NoData,
    NoFix,
    Fresh,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Identity {
    Unknown,
    Pair,
    Pdt,
    Conflicting,
}
impl Identity {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Pair => "pair",
            Self::Pdt => "pdt",
            Self::Conflicting => "conflicting",
        }
    }
}
impl FixState {
    pub const fn name(self) -> &'static str {
        match self {
            Self::NoData => "no_data",
            Self::NoFix => "no_fix",
            Self::Fresh => "fresh",
            Self::Stale => "stale",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub state: FixState,
    pub identity: Identity,
    pub latitude_e7: Option<i32>,
    pub longitude_e7: Option<i32>,
    pub satellites: Option<u8>,
    pub utc: Option<[u8; 6]>,
    pub age_ms: Option<u64>,
    pub bytes: u32,
    pub valid_sentences: u32,
    pub checksum_errors: u32,
    pub parse_errors: u32,
    pub overflows: u32,
    pub line_overflows: u32,
    pub uart_errors: u32,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: FixState::NoData,
            identity: Identity::Unknown,
            latitude_e7: None,
            longitude_e7: None,
            satellites: None,
            utc: None,
            age_ms: None,
            bytes: 0,
            valid_sentences: 0,
            checksum_errors: 0,
            parse_errors: 0,
            overflows: 0,
            line_overflows: 0,
            uart_errors: 0,
        }
    }
}
