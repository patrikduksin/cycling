//! Physical input observations, without gesture policy.
use crate::capability::capability;
use crate::observation::{Availability, Observation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

/// What a button does on every screen (see `docs/ui.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Act,
    Back,
    Move,
}

impl Role {
    pub const ALL: [Role; 3] = [Self::Act, Self::Back, Self::Move];
}

/// The side of the panel a button sits beside.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// The largest [`Fixture::at`]: the far end of its side.
pub const ALONG: u16 = 1000;

/// One physical button: the id its driver reports, its role, if any, and
/// where it sits beside the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fixture {
    pub id: u8,
    pub role: Option<Role>,
    pub edge: Side,
    /// How far along `edge` the button sits, in thousandths of the side from
    /// its top end (left and right sides) or its left end (top and bottom),
    /// so it holds at any panel size. At most [`ALONG`].
    pub at: u16,
}

pub const MAX_BUTTONS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fixtures {
    fixtures: [Fixture; MAX_BUTTONS],
    len: u8,
}

impl Fixtures {
    pub const NONE: Self = Self::new(&[]);

    pub const fn new(list: &[Fixture]) -> Self {
        assert!(list.len() <= MAX_BUTTONS, "more buttons than MAX_BUTTONS");
        let mut fixtures = [Fixture {
            id: 0,
            role: None,
            edge: Side::Left,
            at: 0,
        }; MAX_BUTTONS];
        let mut index = 0;
        while index < list.len() {
            assert!(
                list[index].at <= ALONG,
                "a button sits past the end of its side"
            );
            fixtures[index] = list[index];
            index += 1;
        }
        Self {
            fixtures,
            len: list.len() as u8,
        }
    }

    pub const fn as_slice(&self) -> &[Fixture] {
        self.fixtures.split_at(self.len as usize).0
    }

    pub fn role(&self, id: u8) -> Option<Role> {
        self.as_slice()
            .iter()
            .find(|fixture| fixture.id == id)
            .and_then(|fixture| fixture.role)
    }

    pub fn id(&self, role: Role) -> Option<u8> {
        self.as_slice()
            .iter()
            .find(|fixture| fixture.role == Some(role))
            .map(|fixture| fixture.id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    pub touch: Availability,
    pub buttons: Fixtures,
}

#[capability(id = 7)]
pub trait InputSource {
    #[observe]
    fn controls(&self) -> Controls;
    #[take]
    fn take_edge(&mut self) -> Option<Edge>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputSnapshot {
    pub battery: Observation<(u8, u16)>,
    pub power: Observation<u8>,
    pub button_counts: [u32; 3],
    pub touch_available: bool,
    pub touch_errors: u32,
    pub companion_valid: u32,
    pub companion_bad_crc: u32,
    pub uart_errors: u32,
    pub input_lost: u32,
}

#[capability(id = 8)]
pub trait InputObservation {
    #[observe]
    fn snapshot(&self, #[clock] now_ms: u64) -> Option<InputSnapshot>;
}

/// A contact with no report for this long is over (see [`Input::Cancel`]).
pub const STALE_CONTACT_MS: u64 = 250;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// A finger is down at this point: the first edge of a contact, or a move.
    Touch(Point),
    /// The finger lifted, ending the contact.
    Release,
    /// Cancel all consumer-held input. Caused by invalid/lost touch or edge loss.
    ///
    /// A Cancel during a contact ends that contact for gestures: whatever it
    /// still reports must not complete as a tap or swipe. Producers follow it
    /// with a `Release` once the cancelled contact is over (the panel reports
    /// the lift, or [`STALE_CONTACT_MS`] pass without a report of it), and
    /// send no `Touch` from it before then. Edge queues that overflow keep
    /// this too (`firmware_services::input::Edges`). Consumers that cannot rely on
    /// that start a new contact at a `Touch` arriving at least
    /// `STALE_CONTACT_MS` after the last touch edge.
    Cancel,
    /// A report from the button with this fixture id, with the C606
    /// companion's uninterpreted report code. Repeated code 1 reports do not
    /// establish a physical release; clients choose policy. A new device's
    /// driver produces the C606 companion's codes and repeat cadence.
    Button { id: u8, code: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub sequence: u32,
    pub received_ms: u64,
    pub input: Input,
}
