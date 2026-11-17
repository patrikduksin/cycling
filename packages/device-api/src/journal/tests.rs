use super::*;
use crate::capability::capability;
use core::cell::RefCell;
use std::format;
use std::string::String;
use std::vec::Vec;

#[derive(Default)]
struct Lines {
    pass: u32,
    now: u64,
    open: Option<String>,
    held: Option<String>,
    lines: Vec<String>,
}

impl Lines {
    fn next_pass(&mut self, now: u64) {
        self.pass += 1;
        self.now = now;
    }

    fn word(&mut self, word: String) {
        let line = self.open.as_mut().expect("a record is open");
        line.push(' ');
        line.push_str(&word);
    }
}

impl Journal for Lines {
    fn pass(&self) -> Pass {
        Pass {
            number: self.pass,
            now_ms: self.now,
        }
    }
    fn begin(&mut self, method: MethodId) {
        assert!(self.open.is_none(), "one record at a time");
        self.lines.extend(self.held.take());
        self.open = Some(format!("{}.{}", method.capability, method.operation));
    }
    fn end(&mut self) {
        let line = self.open.take().expect("a record is open");
        self.lines.push(line);
    }
    fn hold(&mut self) {
        self.held = self.open.take();
    }
    fn release(&mut self) -> bool {
        self.held.take().is_some()
    }
    fn repeated(&mut self, method: MethodId, count: u8) {
        self.lines.push(format!(
            "{}.{} repeated {count}",
            method.capability, method.operation
        ));
    }
}

impl<T: core::fmt::Debug + ?Sized> Put<T> for Lines {
    fn put(&mut self, value: &T) {
        self.word(format!("{value:?}"));
    }
}

impl Media<u64, Result<(), u8>> for Lines {
    fn read(&mut self, method: MethodId, at: u64, bytes: &[u8], outcome: &Result<(), u8>) {
        self.lines.push(format!(
            "{}.{} read {at} {bytes:?} {outcome:?}",
            method.capability, method.operation
        ));
    }
    fn wrote(&mut self, method: MethodId, at: u64, bytes: &[u8], outcome: &Result<(), u8>) {
        self.lines.push(format!(
            "{}.{} wrote {at} {bytes:?} {outcome:?}",
            method.capability, method.operation
        ));
    }
    fn erased(&mut self, method: MethodId, at: u64, length: usize, outcome: &Result<(), u8>) {
        self.lines.push(format!(
            "{}.{} erased {at}+{length} {outcome:?}",
            method.capability, method.operation
        ));
    }
}

impl LampRedact for Lines {
    fn unlock(&mut self, slot: &u8, value: &Result<u32, u8>) {
        self.word(format!(
            "{slot} {}",
            value.map(|_| "<key>").unwrap_or("refused")
        ));
    }
    fn owner(&mut self, value: &u32) {
        self.word(format!("<{} digits>", format!("{value}").len()));
    }
}

const SLOTS: [u8; 2] = [4, 9];

#[capability(id = 200)]
trait Lamp {
    #[observe]
    fn level(&self) -> u8;
    #[observe]
    fn age(&self, #[clock] now: u64) -> Option<crate::ant::Snapshot>;
    #[observe]
    fn cells(&self) -> [bool; 3];
    #[observe]
    fn colours(&self) -> &'static [u8];
    #[observe(keys = SLOTS)]
    fn slot(&self, slot: u8) -> bool;
    #[observe(keyed)]
    fn sent(&self, id: u8) -> Option<u8>;
    #[observe(redact)]
    fn owner(&self) -> u32;
    #[request(refreshes)]
    fn set(&mut self, level: u8, #[clock] now: u64) -> bool;
    #[request(redact)]
    fn unlock(&mut self, slot: u8) -> Result<u32, u8>;
    #[take]
    fn next(&mut self) -> Option<u8>;
    #[take(refreshes)]
    fn drain(&mut self, id: u8, #[out] out: &mut [u8]) -> usize;
    #[sink]
    fn show(&mut self, colour: u8, row: impl FnMut(usize) -> u8) -> Result<(), u8>;
    #[sink]
    fn send(&mut self, #[input] bytes: &[u8]) -> usize;
    #[media(read)]
    fn read(&mut self, #[at] at: u64, #[out] out: &mut [u8; 4]) -> Result<(), u8>;
    #[media(write)]
    fn write(&mut self, #[at] at: u64, #[input] bytes: &[u8]) -> Result<(), u8>;
    #[media(erase)]
    fn erase(&mut self, #[at] at: u64, #[len] length: usize) -> Result<(), u8>;
    #[derived]
    fn bright(&self) -> bool;
    #[unrecorded("a test's own switch")]
    fn poke(&mut self, level: u8);
    #[request(refreshes, repeats)]
    fn keep(&mut self, level: u8) -> bool;
}

fn derived_bright(lamp: &(impl Lamp + ?Sized)) -> bool {
    lamp.level() > 50
}

#[derive(Default)]
struct Bulb {
    level: u8,
    reads: core::cell::Cell<u32>,
    queue: Vec<u8>,
    stream: Vec<u8>,
    refuse: bool,
    shown: Vec<u8>,
    connected_at: Option<u64>,
}

impl Lamp for Bulb {
    fn level(&self) -> u8 {
        self.reads.set(self.reads.get() + 1);
        self.level
    }
    fn age(&self, now: u64) -> Option<crate::ant::Snapshot> {
        let at = self.connected_at?;
        Some(crate::ant::Snapshot {
            scanning: false,
            link: crate::ant::LinkState::Connected,
            selected: None,
            generation: 1,
            packets: 0,
            dropped_packets: 0,
            discovery_overflows: 0,
            transport_losses: 0,
            tx_failures: 0,
            age_ms: Some(now - at),
            stale: false,
        })
    }
    fn cells(&self) -> [bool; 3] {
        [self.level > 10, self.level > 20, self.level > 30]
    }
    fn colours(&self) -> &'static [u8] {
        &[7, 8]
    }
    fn slot(&self, slot: u8) -> bool {
        slot < self.level
    }
    fn sent(&self, id: u8) -> Option<u8> {
        (id == 1).then_some(self.level)
    }
    fn owner(&self) -> u32 {
        123_456
    }
    fn set(&mut self, level: u8, _: u64) -> bool {
        self.level = level;
        true
    }
    fn unlock(&mut self, slot: u8) -> Result<u32, u8> {
        if slot == 4 {
            Ok(0xdead_beef)
        } else {
            Err(slot)
        }
    }
    fn next(&mut self) -> Option<u8> {
        self.queue.pop()
    }
    fn drain(&mut self, _: u8, out: &mut [u8]) -> usize {
        let n = self.stream.len().min(out.len());
        out[..n].copy_from_slice(&self.stream[..n]);
        self.stream.drain(..n);
        self.level = self.level.saturating_add(1);
        n
    }
    fn show(&mut self, _: u8, mut row: impl FnMut(usize) -> u8) -> Result<(), u8> {
        if self.refuse {
            return Err(3);
        }
        self.shown.push(row(0));
        Ok(())
    }
    fn send(&mut self, bytes: &[u8]) -> usize {
        bytes.len() / 2
    }
    fn read(&mut self, at: u64, out: &mut [u8; 4]) -> Result<(), u8> {
        if at == 99 {
            return Err(1);
        }
        *out = [at as u8; 4];
        Ok(())
    }
    fn write(&mut self, _: u64, bytes: &[u8]) -> Result<(), u8> {
        if bytes.is_empty() { Err(2) } else { Ok(()) }
    }
    fn erase(&mut self, _: u64, _: usize) -> Result<(), u8> {
        Ok(())
    }
    fn bright(&self) -> bool {
        derived_bright(self)
    }
    fn poke(&mut self, level: u8) {
        self.level = level;
    }
    fn keep(&mut self, level: u8) -> bool {
        let lit = self.level > 0;
        self.level = level;
        lit
    }
}

type Recorded<'a> = LampRecorder<Bulb, &'a RefCell<Lines>, std::boxed::Box<RefCell<LampObserved>>>;

fn lines(journal: &RefCell<Lines>) -> Vec<String> {
    core::mem::take(&mut journal.borrow_mut().lines)
}

#[test]
fn an_observe_is_frozen_for_the_pass_and_recorded_when_it_changes() {
    let journal = RefCell::new(Lines::default());
    let mut lamp: Recorded = LampRecorder::new(
        Bulb {
            level: 5,
            ..Bulb::default()
        },
        &journal,
        Default::default(),
    );
    assert_eq!((lamp.level(), lamp.level()), (5, 5));
    assert_eq!(lamp.unrecorded().reads.get(), 1, "sampled once");
    assert_eq!(lines(&journal), ["200.0 5"]);

    lamp.poke(6);
    assert_eq!(lamp.level(), 5, "frozen until the next pass");
    assert_eq!(lines(&journal), [""; 0]);

    journal.borrow_mut().next_pass(10);
    assert_eq!(lamp.level(), 6);
    journal.borrow_mut().next_pass(20);
    assert_eq!(lamp.level(), 6);
    assert_eq!(
        lines(&journal),
        ["200.0 6"],
        "an unchanged value is not recorded again"
    );
}

#[test]
fn sample_reads_every_observe_once_and_records_each_in_its_own_shape() {
    let journal = RefCell::new(Lines::default());
    let lamp: Recorded = LampRecorder::new(
        Bulb {
            level: 25,
            ..Bulb::default()
        },
        &journal,
        Default::default(),
    );
    lamp.sample();
    assert_eq!(
        lines(&journal),
        [
            "200.0 25",
            "200.1 None",
            "200.2 0 true",
            "200.2 1 true",
            "200.2 2 false",
            "200.3 2 7 8",
            "200.4 4 true",
            "200.4 9 true",
            "200.6 <6 digits>",
        ]
    );
    lamp.sample();
    assert_eq!(
        lines(&journal),
        [""; 0],
        "nothing is sampled twice in a pass"
    );
    assert_eq!(lamp.cells(), [true, true, false]);
    assert_eq!(lamp.colours(), [7, 8]);
    assert!(lamp.slot(9) && lamp.slot(4));
    assert!(!lamp.bright(), "derived from the frozen level");
    assert_eq!(lamp.unrecorded().reads.get(), 1);
}

#[test]
fn an_observe_with_a_clock_is_sampled_at_the_pass_and_ages_between_records() {
    let journal = RefCell::new(Lines::default());
    journal.borrow_mut().next_pass(1_000);
    let lamp: Recorded = LampRecorder::new(
        Bulb {
            connected_at: Some(400),
            ..Bulb::default()
        },
        &journal,
        Default::default(),
    );
    assert_eq!(
        lamp.age(0).unwrap().age_ms,
        Some(600),
        "the pass's time, not the caller's"
    );
    assert_eq!(lines(&journal).len(), 1);
    journal.borrow_mut().next_pass(1_250);
    assert_eq!(lamp.age(0).unwrap().age_ms, Some(850));
    assert_eq!(
        lines(&journal),
        [""; 0],
        "the recorded value aged reads the same"
    );
}

#[test]
fn an_observe_keyed_by_what_vana_asks_is_recorded_each_time_it_is_asked() {
    let journal = RefCell::new(Lines::default());
    let lamp: Recorded = LampRecorder::new(
        Bulb {
            level: 3,
            ..Bulb::default()
        },
        &journal,
        Default::default(),
    );
    assert_eq!((lamp.sent(1), lamp.sent(2)), (Some(3), None));
    assert_eq!(lines(&journal), ["200.5 1 Some(3)", "200.5 2 None"]);
}

#[test]
fn a_request_records_its_arguments_and_answer_then_samples_again() {
    let journal = RefCell::new(Lines::default());
    let mut lamp: Recorded = LampRecorder::new(Bulb::default(), &journal, Default::default());
    assert_eq!(lamp.level(), 0);
    lines(&journal);
    assert!(lamp.set(40, 77));
    assert_eq!(
        lines(&journal),
        ["200.7 40 true", "200.0 40"],
        "the clock is not recorded, and only what the pass had sampled is sampled again"
    );
    assert_eq!(
        lamp.level(),
        40,
        "the rest of the pass sees the request taken"
    );
}

#[test]
fn a_repeating_request_is_recorded_when_it_differs_or_changes_an_observation() {
    let journal = RefCell::new(Lines::default());
    let mut lamp: Recorded = LampRecorder::new(
        Bulb {
            level: 5,
            ..Bulb::default()
        },
        &journal,
        Default::default(),
    );
    lamp.level();
    assert!(lamp.keep(3));
    assert_eq!(lines(&journal), ["200.0 5", "200.18 3 true", "200.0 3"]);

    journal.borrow_mut().next_pass(10);
    lamp.level();
    assert!(lamp.keep(3));
    assert_eq!(
        lines(&journal),
        [""; 0],
        "the same call, and nothing changed"
    );

    journal.borrow_mut().next_pass(20);
    lamp.poke(9);
    lamp.level();
    assert!(lamp.keep(3));
    assert_eq!(
        lines(&journal),
        ["200.0 9", "200.18 3 true", "200.0 3"],
        "the same call changed what the lamp shows"
    );

    assert!(lamp.keep(3));
    lamp.poke(0);
    assert!(!lamp.keep(3));
    assert_eq!(
        lines(&journal),
        ["200.18 3 false", "200.18 repeated 1"],
        "a record after an unrecorded call of the pass says how many"
    );

    assert!(lamp.keep(4));
    assert_eq!(lines(&journal), ["200.18 4 true", "200.0 4"]);
}

#[test]
fn a_redacted_method_is_recorded_by_its_hook() {
    let journal = RefCell::new(Lines::default());
    let mut lamp: Recorded = LampRecorder::new(Bulb::default(), &journal, Default::default());
    assert_eq!(lamp.unlock(4), Ok(0xdead_beef));
    assert_eq!(lamp.unlock(5), Err(5));
    assert_eq!(lamp.owner(), 123_456);
    assert_eq!(
        lines(&journal),
        ["200.8 4 <key>", "200.8 5 refused", "200.6 <6 digits>"]
    );
}

#[test]
fn a_take_records_what_it_gave_and_nothing_for_an_empty_poll() {
    let journal = RefCell::new(Lines::default());
    let mut lamp: Recorded = LampRecorder::new(
        Bulb {
            queue: [8].into(),
            stream: (0..=255).cycle().take(PART + 2).collect(),
            ..Bulb::default()
        },
        &journal,
        Default::default(),
    );
    assert_eq!((lamp.next(), lamp.next()), (Some(8), None));
    assert_eq!(lines(&journal), ["200.9 8"]);

    let mut out = [0u8; 2 * PART];
    assert_eq!(lamp.drain(6, &mut out), PART + 2);
    let recorded = lines(&journal);
    assert_eq!(recorded.len(), 2, "a long read splits into parts");
    assert!(recorded[0].starts_with("200.10 6 true [0, 1, 2,"));
    assert_eq!(recorded[1], "200.10 6 false [0, 1]");
    assert_eq!(lamp.drain(6, &mut out), 0);
    assert_eq!(lines(&journal), ["200.10 6 false []"]);
}

#[test]
fn a_sink_records_a_refusal_at_its_attempt_and_always_a_count() {
    let journal = RefCell::new(Lines::default());
    let mut lamp: Recorded = LampRecorder::new(Bulb::default(), &journal, Default::default());
    assert_eq!(lamp.show(1, |_| 9), Ok(()));
    lamp.unrecorded_mut().refuse = true;
    assert_eq!(lamp.show(2, |_| 9), Err(3));
    assert_eq!(
        lamp.unrecorded().shown,
        [9],
        "the callback reaches the device"
    );
    assert_eq!(lines(&journal), ["200.11 1 2 Err(3)"]);

    journal.borrow_mut().next_pass(5);
    assert_eq!(lamp.show(2, |_| 9), Err(3));
    assert_eq!(lamp.send(&[1, 2, 3, 4]), 2);
    assert_eq!(
        lines(&journal),
        ["200.11 0 2 Err(3)", "200.12 0 4 2"],
        "attempts count from zero each pass"
    );
}

#[test]
fn media_goes_to_the_journal_with_its_address_bytes_and_outcome() {
    let journal = RefCell::new(Lines::default());
    let mut lamp: Recorded = LampRecorder::new(Bulb::default(), &journal, Default::default());
    let mut out = [0u8; 4];
    assert_eq!(lamp.read(7, &mut out), Ok(()));
    assert_eq!(lamp.read(99, &mut out), Err(1));
    assert_eq!(lamp.write(8, &[1, 2]), Ok(()));
    assert_eq!(lamp.write(8, &[]), Err(2));
    assert_eq!(lamp.erase(16, 32), Ok(()));
    assert_eq!(
        lines(&journal),
        [
            "200.13 read 7 [7, 7, 7, 7] Ok(())",
            "200.13 read 99 [7, 7, 7, 7] Err(1)",
            "200.14 wrote 8 [1, 2] Ok(())",
            "200.14 wrote 8 [] Err(2)",
            "200.15 erased 16+32 Ok(())",
        ]
    );
}

#[test]
fn a_lent_device_is_the_device() {
    let mut bulb = Bulb {
        level: 70,
        ..Bulb::default()
    };
    fn bright(lamp: impl Lamp) -> bool {
        lamp.bright()
    }
    assert!(bright(&mut bulb));
}
