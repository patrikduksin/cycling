use super::*;
use Button::{BottomLeft as MOVE, BottomRight as ACT, TopLeft as BACK};
use Format::*;
use std::vec;
use std::vec::Vec;

fn scope(stores: &[Store]) -> Scope {
    Scope::of(stores.iter().copied()).unwrap()
}

const RIDES: Scope = Scope(1 << Store::Rides as u8);
const INTERNAL: Scope = Scope(1 << Store::Internal as u8);
const INTERNAL_RIDES: Scope = Scope(INTERNAL.0 | RIDES.0);
const ASKED: Format = Offered {
    drawn_at: Some(1_000),
    scope: RIDES,
};
const UNDRAWN: Format = Offered {
    drawn_at: None,
    scope: RIDES,
};

fn press(button: Button, at: u64) -> Event {
    Event::Report {
        button,
        held: false,
        at,
    }
}

fn held(button: Button, at: u64) -> Event {
    Event::Report {
        button,
        held: true,
        at,
    }
}

fn read(foreign: bool, hub: bool) -> Event {
    Event::Read {
        foreign: foreign.then_some(RIDES),
        hub,
        card: CardAccess::Writable,
    }
}

const WITHOUT_CARD: Event = Event::Read {
    foreign: None,
    hub: true,
    card: CardAccess::Out,
};

fn confirm(scope: Scope) -> Event {
    Event::Confirm { scope, clear: None }
}

fn running(stage: Stage, scope: Scope) -> Format {
    Running {
        stage,
        scope,
        resumed: false,
        clear: None,
    }
}

fn resumed_at(stage: Stage, scope: Scope) -> Format {
    Running {
        stage,
        scope,
        resumed: true,
        clear: None,
    }
}

#[test]
fn every_state_and_event_steps_as_the_table_says() {
    let every = Scope::every(CardAccess::Writable);
    let wiping = running(Stage::FIRST, every);
    let rides = running(Stage::Card(Card::Rides), RIDES);
    let resuming = Resuming { scope: every };
    let begun = running(Stage::Card(Card::Table), every);
    let resumed = resumed_at(Stage::Card(Card::Table), every);
    let last_sector = Stage::JournalSector(SECTORS - 1);
    let rows = [
        (Unread, Event::IntentAtBoot(Found::Idle), Idle, Go::Stay),
        (
            Unread,
            Event::IntentAtBoot(Found::Pending(every)),
            resuming,
            Go::Stay,
        ),
        (
            Unread,
            Event::IntentAtBoot(Found::ForeignBytes),
            Unread,
            Go::Stay,
        ),
        (
            Unread,
            Event::IntentAtBoot(Found::ReadFailed),
            Unread,
            Go::Stay,
        ),
        (Unread, read(true, true), Failed, Go::Ready { clear: None }),
        (
            Unread,
            read(false, false),
            Failed,
            Go::Ready { clear: None },
        ),
        (Unread, confirm(every), Unread, Go::Stay),
        (Unread, press(ACT, 9_000), Unread, Go::Stay),
        (resuming, read(false, true), resumed, Go::Question),
        (resuming, read(true, true), resumed, Go::Question),
        (resuming, read(true, false), resumed, Go::Question),
        (
            resuming,
            WITHOUT_CARD,
            resumed_at(Stage::JournalSector(0), Scope::every(CardAccess::Out)),
            Go::Question,
        ),
        (
            Resuming { scope: RIDES },
            WITHOUT_CARD,
            resumed_at(Stage::SettleIntent, RIDES),
            Go::Question,
        ),
        (resuming, Event::Drawn(1_000), resuming, Go::Stay),
        (resuming, press(ACT, 9_000), resuming, Go::Stay),
        (resuming, press(BACK, 9_000), resuming, Go::Stay),
        (resuming, confirm(every), resuming, Go::Stay),
        (
            resuming,
            Event::IntentAtBoot(Found::Idle),
            resuming,
            Go::Stay,
        ),
        (resumed, press(BACK, 9_000), resumed, Go::Stay),
        (
            Idle,
            Event::IntentAtBoot(Found::Pending(every)),
            Idle,
            Go::Stay,
        ),
        (Idle, read(true, true), UNDRAWN, Go::Question),
        (Idle, read(false, true), Idle, Go::Stay),
        (Idle, read(true, false), Idle, Go::Stay),
        (Declined, read(true, true), Declined, Go::Stay),
        (Failed, read(true, true), Failed, Go::Stay),
        (UNDRAWN, Event::Drawn(1_000), ASKED, Go::Stay),
        (ASKED, Event::Drawn(5_000), ASKED, Go::Stay),
        (UNDRAWN, press(ACT, 9_000), UNDRAWN, Go::Stay),
        (ASKED, press(ACT, 900), ASKED, Go::Stay),
        (ASKED, press(ACT, 1_249), ASKED, Go::Stay),
        (ASKED, held(ACT, 9_000), ASKED, Go::Stay),
        (ASKED, held(BACK, 9_000), ASKED, Go::Stay),
        (ASKED, press(BACK, 1_100), ASKED, Go::Stay),
        (ASKED, press(MOVE, 9_000), ASKED, Go::Stay),
        (
            ASKED,
            press(ACT, 1_250),
            running(Stage::RecordIntent, RIDES),
            Go::Question,
        ),
        (
            running(Stage::RecordIntent, RIDES),
            Event::IntentRecorded(RIDES),
            rides,
            Go::Stay,
        ),
        (
            running(Stage::RecordIntent, INTERNAL),
            Event::IntentRecorded(INTERNAL_RIDES),
            running(Stage::Card(Card::Rides), INTERNAL_RIDES),
            Go::Stay,
        ),
        (ASKED, press(BACK, 1_250), Declined, Go::FirstPage),
        (Idle, confirm(every), wiping, Go::Question),
        (ASKED, confirm(every), wiping, Go::Question),
        (Declined, confirm(every), wiping, Go::Question),
        (Failed, confirm(every), wiping, Go::Question),
        (wiping, confirm(every), wiping, Go::Stay),
        (wiping, press(ACT, 9_000), wiping, Go::Stay),
        (wiping, Event::IntentRecorded(every), begun, Go::Stay),
        (
            rides,
            Event::Wrote,
            running(Stage::SettleIntent, RIDES),
            Go::Stay,
        ),
        (
            running(Stage::Settings, every),
            Event::Wrote,
            running(Stage::SettleIntent, every),
            Go::Stay,
        ),
        (
            running(last_sector, every),
            Event::Wrote,
            running(Stage::Settings, every),
            Go::Stay,
        ),
        (
            resumed_at(last_sector, every),
            Event::Wrote,
            resumed_at(Stage::Settings, every),
            Go::Stay,
        ),
        (
            running(last_sector, INTERNAL),
            Event::Wrote,
            running(Stage::ForgetInternalRides, INTERNAL),
            Go::Stay,
        ),
        (
            running(Stage::SettleIntent, INTERNAL),
            Event::Wrote,
            Idle,
            Go::JournalEmptied { clear: None },
        ),
        (
            running(Stage::SettleIntent, INTERNAL_RIDES),
            Event::Wrote,
            Done,
            Go::Stay,
        ),
        (
            running(Stage::SettleIntent, RIDES),
            Event::Wrote,
            Done,
            Go::Stay,
        ),
        (
            wiping,
            Event::IntentNotRecorded,
            Failed,
            Go::Ready { clear: None },
        ),
        (
            begun,
            Event::WriteFailed,
            running(Stage::Abandon, every),
            Go::Stay,
        ),
        (
            running(Stage::SettleIntent, RIDES),
            Event::WriteFailed,
            running(Stage::Abandon, RIDES),
            Go::Stay,
        ),
        (
            running(Stage::Abandon, RIDES),
            Event::Wrote,
            Failed,
            Go::Ready { clear: None },
        ),
        (
            running(Stage::Abandon, RIDES),
            Event::WriteFailed,
            Failed,
            Go::Ready { clear: None },
        ),
        (Done, confirm(every), Done, Go::Stay),
        (Done, read(true, true), Done, Go::Stay),
    ];
    for (format, event, after, go) in rows {
        assert_eq!(
            step(format, event, false),
            (after, go),
            "{format:?} {event:?}"
        );
    }
}

#[test]
fn a_clear_keeps_its_token_through_the_wipe_and_hands_it_to_the_ending() {
    const TOKEN: u32 = 7;
    let clearing = |stage| Running {
        stage,
        scope: INTERNAL,
        resumed: false,
        clear: Some(TOKEN),
    };
    let clear = Event::Confirm {
        scope: INTERNAL,
        clear: Some(TOKEN),
    };
    assert_eq!(
        step(Idle, clear, false),
        (clearing(Stage::RecordIntent), Go::Question)
    );
    let stages = [
        Stage::RecordIntent,
        Stage::ForgetInternalRides,
        Stage::SettleIntent,
        Stage::Abandon,
    ]
    .into_iter()
    .chain((0..SECTORS).map(Stage::JournalSector));
    let inert = [
        clear,
        confirm(INTERNAL),
        read(true, true),
        Event::Drawn(1_000),
        press(ACT, 9_000),
    ];
    let mut endings = Vec::new();
    for stage in stages {
        let written = match stage {
            Stage::RecordIntent => [Event::IntentRecorded(INTERNAL), Event::IntentNotRecorded],
            _ => [Event::Wrote, Event::WriteFailed],
        };
        for event in written.into_iter().chain(inert) {
            let (after, go) = step(clearing(stage), event, false);
            match (after, go) {
                (Running { clear, .. }, Go::Stay) => assert_eq!(clear, Some(TOKEN)),
                (_, Go::Ready { clear } | Go::JournalEmptied { clear }) => {
                    assert_eq!(clear, Some(TOKEN), "{stage:?} {event:?}");
                    endings.push(go);
                }
                _ => panic!("{stage:?} {event:?} gave {after:?} {go:?}"),
            }
        }
    }
    assert!(endings.contains(&Go::Ready { clear: Some(TOKEN) }));
    assert!(endings.contains(&Go::JournalEmptied { clear: Some(TOKEN) }));
}

#[test]
fn each_store_keeps_its_bit_in_the_intent_journal() {
    let bits = Store::ALL.map(|store| scope(&[store]).bits());
    assert_eq!(bits, [1, 2, 4, 8, 16, 32, 64]);
    assert_eq!(Scope::every(CardAccess::Writable).bits(), 0x7f);
    assert_eq!(Scope::every(CardAccess::Out).bits(), 0x60);
    assert_eq!(
        Scope::from_bits(0x7f),
        Some(Scope::every(CardAccess::Writable))
    );
    assert_eq!(Scope::from_bits(0), None);
    assert_eq!(Scope::from_bits(0x80), None);
    assert_eq!(Scope::from_bits(0xa0), None);
}

#[test]
fn no_ride_starts_before_the_intent_is_read_or_while_a_wipe_resumes() {
    let every = Scope::every(CardAccess::Writable);
    for format in [
        Unread,
        Resuming { scope: every },
        running(Stage::Card(Card::Table), every),
        running(Stage::SettleIntent, every),
        Failed,
        Done,
    ] {
        assert!(!format.admits_rides(), "{format:?}");
    }
    assert!(Idle.admits_rides());
    assert!(Declined.admits_rides());
    for format in [Unread, Resuming { scope: every }, Failed, Done] {
        assert!(format.holds_storage(), "{format:?}");
    }
    for format in [Idle, ASKED, Declined] {
        assert!(!format.holds_storage(), "{format:?}");
    }
    assert!(
        !Resuming { scope: every }.owns_storage(),
        "boot scans go on"
    );
}

#[test]
fn nothing_starts_a_wipe_under_an_open_ride() {
    let every = confirm(Scope::every(CardAccess::Writable));
    for (format, event) in [
        (ASKED, press(ACT, 1_250)),
        (Idle, every),
        (UNDRAWN, every),
        (Declined, every),
        (Failed, every),
    ] {
        assert_eq!(step(format, event, true), (format, Go::Stay), "{format:?}");
    }
    assert_eq!(
        step(ASKED, press(BACK, 1_250), true),
        (Declined, Go::FirstPage)
    );
}

#[test]
fn a_wipe_takes_the_stages_of_the_stores_it_is_for_in_order() {
    use Card::{Files, Rides, Table, Traces};
    let journal = || (0..SECTORS).map(Stage::JournalSector);
    let card = [Table, Rides, Files, Traces].map(Stage::Card);
    let cases: [(Scope, Vec<Stage>); 12] = [
        (scope(&[Store::Table]), card.to_vec()),
        (scope(&[Store::Rides]), vec![Stage::Card(Rides)]),
        (scope(&[Store::Files]), vec![Stage::Card(Files)]),
        (scope(&[Store::Records]), vec![Stage::Card(Files)]),
        (scope(&[Store::Traces]), vec![Stage::Card(Traces)]),
        (
            scope(&[Store::Internal]),
            journal().chain([Stage::ForgetInternalRides]).collect(),
        ),
        (
            scope(&[Store::Internal, Store::Rides]),
            [Stage::Card(Rides)]
                .into_iter()
                .chain(journal())
                .chain([Stage::ForgetInternalRides])
                .collect(),
        ),
        (
            scope(&[Store::Internal, Store::Records]),
            [Stage::Card(Files)].into_iter().chain(journal()).collect(),
        ),
        (scope(&[Store::Settings]), vec![Stage::Settings]),
        (
            scope(&[Store::Settings, Store::Rides]),
            vec![Stage::Card(Rides), Stage::Settings],
        ),
        (
            Scope::every(CardAccess::Writable),
            card.into_iter()
                .chain(journal())
                .chain([Stage::Settings])
                .collect(),
        ),
        (
            Scope::every(CardAccess::Out),
            journal()
                .chain([Stage::ForgetInternalRides, Stage::Settings])
                .collect(),
        ),
    ];
    for (scope, stages) in cases {
        let wipe = |format| {
            let mut taken = Vec::new();
            let mut format = step(format, confirm(scope), false).0;
            while let Running { stage, .. } = format {
                taken.push(stage);
                let event = match stage {
                    Stage::RecordIntent => Event::IntentRecorded(scope),
                    _ => Event::Wrote,
                };
                format = step(format, event, false).0;
            }
            let ended = if scope == INTERNAL { Idle } else { Done };
            assert_eq!(format, ended, "{scope:?}");
            taken
        };
        let mut begun = vec![Stage::RecordIntent];
        begun.extend(stages.iter().copied());
        begun.push(Stage::SettleIntent);
        assert_eq!(wipe(Idle), begun, "{scope:?}");
        let resumed = step(Unread, Event::IntentAtBoot(Found::Pending(scope)), false).0;
        let resumed = step(resumed, read(false, true), false).0;
        assert_eq!(wipe(resumed), begun[1..], "{scope:?} resumed");
    }
}

#[test]
fn the_question_names_what_goes_by_the_gravest_store() {
    use Store::*;
    let cases: [(&[Store], Goes); 17] = [
        (&[Rides, Settings], Goes::RidesAndSettings),
        (&[Table, Settings, Internal], Goes::RidesAndSettings),
        (&[Table], Goes::WholeCard),
        (&[Rides], Goes::CardRides),
        (&[Files], Goes::CardFiles),
        (&[Records], Goes::CardFiles),
        (&[Files, Records], Goes::CardFiles),
        (&[Traces], Goes::Traces),
        (&[Internal], Goes::RideLog),
        (&[Settings], Goes::Settings),
        (&[Rides, Internal], Goes::RidesAndMore),
        (&[Table, Rides], Goes::RidesAndMore),
        (&[Settings, Records], Goes::SettingsAndMore),
        (&[Settings, Traces], Goes::SettingsAndMore),
        (&[Files, Internal], Goes::FilesAndMore),
        (&[Records, Traces], Goes::FilesAndMore),
        (&[Internal, Traces], Goes::RideLogAndMore),
    ];
    for (stores, goes) in cases {
        assert_eq!(scope(stores).goes(), goes, "{stores:?}");
    }
}
