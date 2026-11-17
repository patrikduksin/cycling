/// Where a recorder keeps its rides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Medium {
    /// The 1 MiB journal in the owned boot-flash reservation.
    Internal,
    /// The sector log in the owned MMC partition.
    Bulk,
}

impl Medium {
    /// How workout records and ride facts on FILES name this journal.
    pub const fn journal(self) -> u8 {
        match self {
            Self::Internal => b'i',
            Self::Bulk => b'b',
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Bulk => "bulk",
        }
    }

    /// Short label for the screen.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Internal => "INT",
            Self::Bulk => "MMC",
        }
    }
}
