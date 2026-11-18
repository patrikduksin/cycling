//! The derived owner log: `why!("cue.held", cue, held = self.playing)` notes
//! a decision, by a static tag, and the operands it was made from, each named
//! by its identifier or by `name = expr`.
//!
//! On the device, operands are type-checked but never evaluated.
//! On host, operands are evaluated and formatted only while [`collect`] is active.

#[cfg(not(target_os = "none"))]
extern crate std;

#[cfg(target_os = "none")]
#[macro_export]
macro_rules! why {
    ($tag:expr $(, $name:ident $(= $value:expr)?)* $(,)?) => {
        if false {
            let _ = ($(&$crate::__why_operand!($name $(= $value)?),)*);
        }
    };
}

#[cfg(not(target_os = "none"))]
#[macro_export]
macro_rules! why {
    ($tag:expr $(, $name:ident $(= $value:expr)?)* $(,)?) => {
        if $crate::why::collecting() {
            $crate::why::note(
                $tag,
                &[$((
                    ::core::stringify!($name),
                    &$crate::__why_operand!($name $(= $value)?) as &dyn ::core::fmt::Debug,
                )),*],
            );
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __why_operand {
    ($name:ident) => {
        $name
    };
    ($name:ident = $value:expr) => {
        $value
    };
}

#[cfg(not(target_os = "none"))]
pub use collector::*;

#[cfg(not(target_os = "none"))]
mod collector {
    use super::std;
    use super::{Fields, Value};
    use core::fmt::Write as _;
    use core::hash::{Hash, Hasher};
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::fmt::Debug;
    use std::string::String;
    use std::vec::Vec;

    /// One decision, at the pass it was noted in.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Why {
        pub at_ms: u64,
        pub tag: &'static str,
        pub operands: Vec<(&'static str, String)>,
    }

    /// One field of an owner's state that a pass changed, as text;
    /// `None` on the side where the owner did not show the field.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Delta {
        pub at_ms: u64,
        pub owner: &'static str,
        pub field: &'static str,
        pub old: Option<String>,
        pub new: Option<String>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Entry {
        Decision(Why),
        Delta(Delta),
    }

    impl Entry {
        pub fn at_ms(&self) -> u64 {
            match self {
                Entry::Decision(why) => why.at_ms,
                Entry::Delta(delta) => delta.at_ms,
            }
        }
    }

    #[derive(Default)]
    struct Collector {
        at_ms: u64,
        pass: u64,
        entries: Vec<Entry>,
        last: BTreeMap<&'static str, LastNote>,
        window: Option<(u64, u64)>,
        shown: Option<BTreeMap<(&'static str, &'static str), Shown>>,
    }

    struct LastNote {
        entry: usize,
        noted_pass: u64,
    }

    struct Shown {
        digest: u64,
        text: String,
        pass: u64,
    }

    std::thread_local! {
        static ON: Cell<bool> = const { Cell::new(false) };
        static COLLECTOR: RefCell<Collector> = RefCell::new(Collector::default());
    }

    /// Collecting on this thread until the guard drops.
    #[must_use = "collecting stops when the guard drops"]
    pub struct Collecting(());

    pub fn collect() -> Collecting {
        COLLECTOR.with_borrow_mut(|collector| *collector = Collector::default());
        ON.set(true);
        Collecting(())
    }

    impl Drop for Collecting {
        fn drop(&mut self) {
            ON.set(false);
            COLLECTOR.with_borrow_mut(|collector| *collector = Collector::default());
        }
    }

    pub fn collecting() -> bool {
        ON.get()
    }

    /// [`deltas`] records the passes from `from_ms` to `to_ms`, both
    /// included.
    pub fn deltas_in(from_ms: u64, to_ms: u64) {
        COLLECTOR.with_borrow_mut(|collector| collector.window = Some((from_ms, to_ms)));
    }

    /// A pass begins at `now_ms`: what is noted until the next one is
    /// stamped with it. The [`deltas_in`] window's first pass reads `state`
    /// here, before it runs, as the baseline its changes are told against.
    pub fn pass(now_ms: u64, state: impl FnOnce(&mut Fields<'_>)) {
        if !collecting() {
            return;
        }
        let baseline = COLLECTOR.with_borrow_mut(|collector| {
            collector.at_ms = now_ms;
            collector.pass += 1;
            collector.shown.is_none() && collector.in_window()
        });
        if baseline {
            show(state, Reading::Baseline);
        }
    }

    /// A decision re-made each pass from the same operands is one entry:
    /// an entry equal to its tag's last one is dropped while that one was
    /// noted this pass or the pass before.
    pub fn note(tag: &'static str, operands: &[(&'static str, &dyn Debug)]) {
        let operands: Vec<(&'static str, String)> = operands
            .iter()
            .map(|(name, value)| (*name, std::format!("{value:?}")))
            .collect();
        COLLECTOR.with_borrow_mut(|collector| {
            let pass = collector.pass;
            if let Some(last) = collector.last.get_mut(tag)
                && last.noted_pass + 1 >= pass
                && matches!(&collector.entries[last.entry], Entry::Decision(why) if why.operands == operands)
            {
                last.noted_pass = pass;
                return;
            }
            let entry = collector.entries.len();
            collector.last.insert(
                tag,
                LastNote {
                    entry,
                    noted_pass: pass,
                },
            );
            let at_ms = collector.at_ms;
            collector.entries.push(Entry::Decision(Why {
                at_ms,
                tag,
                operands,
            }));
        });
    }

    /// At the end of a pass in the [`deltas_in`] window, the fields `state`
    /// shows that differ from the pass before, each an entry. Outside the
    /// window `state` is not called.
    pub fn deltas(state: impl FnOnce(&mut Fields<'_>)) {
        if collecting() && COLLECTOR.with_borrow(Collector::in_window) {
            show(state, Reading::Changes);
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Reading {
        Baseline,
        Changes,
    }

    fn show(state: impl FnOnce(&mut Fields<'_>), reading: Reading) {
        let mut sink = |owner, field, value: Value<'_>| {
            COLLECTOR.with_borrow_mut(|collector| collector.show(owner, field, &value, reading));
        };
        state(&mut Fields::new(&mut sink));
        COLLECTOR.with_borrow_mut(|collector| match reading {
            Reading::Baseline => {
                collector.shown.get_or_insert_default();
            }
            Reading::Changes => collector.hide_unshown(),
        });
    }

    impl Collector {
        fn in_window(&self) -> bool {
            (self.window).is_some_and(|(from, to)| (from..=to).contains(&self.at_ms))
        }

        fn show(
            &mut self,
            owner: &'static str,
            field: &'static str,
            value: &Value<'_>,
            reading: Reading,
        ) {
            let pass = match reading {
                Reading::Baseline => self.pass - 1,
                Reading::Changes => self.pass,
            };
            let at_ms = self.at_ms;
            let digest = digest(value);
            let shown = self.shown.get_or_insert_default();
            let old = match shown.get_mut(&(owner, field)) {
                Some(last) if last.digest == digest => {
                    last.pass = pass;
                    return;
                }
                Some(last) => Some(core::mem::take(&mut last.text)),
                None => None,
            };
            let text = text(value);
            let new = Some(text.clone());
            shown.insert((owner, field), Shown { digest, text, pass });
            if reading == Reading::Changes {
                self.entries.push(Entry::Delta(Delta {
                    at_ms,
                    owner,
                    field,
                    old,
                    new,
                }));
            }
        }

        fn hide_unshown(&mut self) {
            let (pass, at_ms) = (self.pass, self.at_ms);
            let shown = self.shown.get_or_insert_default();
            let entries = &mut self.entries;
            shown.retain(|&(owner, field), last| {
                if last.pass == pass {
                    return true;
                }
                entries.push(Entry::Delta(Delta {
                    at_ms,
                    owner,
                    field,
                    old: Some(core::mem::take(&mut last.text)),
                    new: None,
                }));
                false
            });
        }
    }

    struct Hashing<'a>(&'a mut std::hash::DefaultHasher);

    impl core::fmt::Write for Hashing<'_> {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            self.0.write(text.as_bytes());
            Ok(())
        }
    }

    fn digest(value: &Value<'_>) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        match value {
            Value::Debug(value) => {
                let _ = write!(Hashing(&mut hasher), "{value:?}");
            }
            Value::Buffer { len, digest } => (len, digest).hash(&mut hasher),
        }
        hasher.finish()
    }

    fn text(value: &Value<'_>) -> String {
        match value {
            Value::Debug(value) => std::format!("{value:?}"),
            Value::Buffer { len, digest } => std::format!("{len} items, digest {digest:016x}"),
        }
    }

    /// The entries so far, in the order they were noted; collecting goes on
    /// from nothing.
    pub fn take() -> Vec<Entry> {
        COLLECTOR.with_borrow_mut(|collector| {
            collector.last.clear();
            core::mem::take(&mut collector.entries)
        })
    }
}

/// An owner's state, field by field, for the deltas a `cycling trace why` window shows.
#[cfg(not(target_os = "none"))]
pub trait State {
    fn fields(&self, out: &mut Fields<'_>);
}

#[cfg(not(target_os = "none"))]
pub enum Value<'a> {
    Debug(&'a dyn core::fmt::Debug),
    Buffer { len: usize, digest: u64 },
}

#[cfg(not(target_os = "none"))]
pub struct Fields<'a> {
    owner: &'static str,
    sink: &'a mut dyn FnMut(&'static str, &'static str, Value<'_>),
}

#[cfg(not(target_os = "none"))]
impl<'a> Fields<'a> {
    pub fn new(sink: &'a mut dyn FnMut(&'static str, &'static str, Value<'_>)) -> Self {
        Self { owner: "", sink }
    }

    pub fn owner(&mut self, owner: &'static str, state: &impl State) {
        let outer = core::mem::replace(&mut self.owner, owner);
        state.fields(self);
        self.owner = outer;
    }

    pub fn field(&mut self, name: &'static str, value: &dyn core::fmt::Debug) {
        (self.sink)(self.owner, name, Value::Debug(value));
    }

    pub fn buffer<T: core::hash::Hash>(&mut self, name: &'static str, items: &[T]) {
        use core::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        items.hash(&mut hasher);
        let (len, digest) = (items.len(), hasher.finish());
        (self.sink)(self.owner, name, Value::Buffer { len, digest });
    }
}

#[cfg(test)]
mod tests {
    use super::{Delta, Entry, Fields, State, Why};
    use std::string::{String, ToString};
    use std::vec::Vec;

    fn entry(at_ms: u64, tag: &'static str, operands: &[(&'static str, &str)]) -> Entry {
        Entry::Decision(Why {
            at_ms,
            tag,
            operands: operands
                .iter()
                .map(|(name, value)| (*name, value.to_string()))
                .collect(),
        })
    }

    #[test]
    fn a_note_names_its_operands_by_identifier_or_by_name() {
        let _collecting = super::collect();
        super::pass(1_000, |_| {});
        let cue = Some("lap");
        let playing = 3;
        why!("cue.held", cue, held = playing > 2);
        why!("toast.none");
        assert_eq!(
            super::take(),
            [
                entry(
                    1_000,
                    "cue.held",
                    &[("cue", "Some(\"lap\")"), ("held", "true")]
                ),
                entry(1_000, "toast.none", &[]),
            ]
        );
    }

    #[test]
    fn nothing_is_noted_or_formatted_unless_collecting() {
        let formatted = core::cell::Cell::new(false);
        struct Spy<'a>(&'a core::cell::Cell<bool>);
        impl core::fmt::Debug for Spy<'_> {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                self.0.set(true);
                f.write_str("spy")
            }
        }
        super::pass(5, |_| {});
        why!("toast.lap", lap = Spy(&formatted));
        assert!(!formatted.get(), "formatted while not collecting");
        let collecting = super::collect();
        assert_eq!(super::take(), [], "noted before collecting");
        drop(collecting);
        why!("toast.lap", lap = Spy(&formatted));
        let _collecting = super::collect();
        assert_eq!(super::take(), [], "noted after collecting stopped");
        assert!(!formatted.get());
    }

    #[test]
    fn a_decision_held_across_passes_is_one_entry_and_one_that_returns_is_another() {
        let _collecting = super::collect();
        let page = |at: u64, page: &str| {
            super::pass(at, |_| {});
            why!("toast.none", page);
            why!("toast.none", page);
        };
        for at in [0, 100, 200, 300] {
            page(at, "home");
        }
        page(400, "ride");
        page(500, "home");
        super::pass(600, |_| {});
        super::pass(700, |_| {});
        page(800, "home");
        let noted: Vec<(u64, String)> = super::take()
            .into_iter()
            .map(|entry| match entry {
                Entry::Decision(why) => (why.at_ms, why.operands[0].1.clone()),
                Entry::Delta(delta) => panic!("no state was shown: {delta:?}"),
            })
            .collect();
        let home = || "\"home\"".to_string();
        assert_eq!(
            noted,
            [
                (0, home()),
                (400, "\"ride\"".to_string()),
                (500, home()),
                (800, home()),
            ]
        );
    }

    struct Follower<'a> {
        against_fixes: u32,
        sign: Option<&'static str>,
        track: &'a [u16],
        joined: Option<u32>,
    }

    impl State for Follower<'_> {
        fn fields(&self, out: &mut Fields<'_>) {
            out.field("against_fixes", &self.against_fixes);
            out.field("sign", &self.sign);
            out.buffer("track", self.track);
            if let Some(segment) = &self.joined {
                out.field("segment", segment);
            }
        }
    }

    fn delta(at_ms: u64, field: &'static str, old: Option<&str>, new: Option<&str>) -> Entry {
        Entry::Delta(Delta {
            at_ms,
            owner: "follow",
            field,
            old: old.map(str::to_string),
            new: new.map(str::to_string),
        })
    }

    #[test]
    fn a_delta_names_only_the_fields_each_pass_of_the_window_changed() {
        let _collecting = super::collect();
        super::deltas_in(1_000, 3_000);
        let track = [1, 2, 3];
        let mut follower = Follower {
            against_fixes: 0,
            sign: None,
            track: &track,
            joined: None,
        };
        for at in [0, 1_000, 2_000, 3_000, 4_000] {
            super::pass(at, |out| out.owner("follow", &follower));
            follower.against_fixes += 1;
            if at == 2_000 {
                why!("follow.turn_around", against_fixes = follower.against_fixes);
                follower.sign = Some("TurnAround");
            }
            if at == 3_000 {
                follower.joined = Some(7);
            }
            super::deltas(|out| out.owner("follow", &follower));
        }
        assert_eq!(
            super::take(),
            [
                delta(1_000, "against_fixes", Some("1"), Some("2")),
                entry(2_000, "follow.turn_around", &[("against_fixes", "3")]),
                delta(2_000, "against_fixes", Some("2"), Some("3")),
                delta(2_000, "sign", Some("None"), Some("Some(\"TurnAround\")")),
                delta(3_000, "against_fixes", Some("3"), Some("4")),
                delta(3_000, "segment", None, Some("7")),
            ],
            "the pass at 1000 is told against the state before it; the one at 4000 is past the window"
        );
    }

    #[test]
    fn the_windows_first_pass_shows_what_it_changed_from_the_pass_before() {
        let _collecting = super::collect();
        super::deltas_in(1_000, 1_000);
        let track = [1, 2, 3];
        let mut follower = Follower {
            against_fixes: 0,
            sign: None,
            track: &track,
            joined: None,
        };
        for at in [0, 500, 1_000, 2_000] {
            super::pass(at, |out| out.owner("follow", &follower));
            if at == 1_000 {
                follower.sign = Some("TurnAround");
            }
            super::deltas(|out| out.owner("follow", &follower));
        }
        assert_eq!(
            super::take(),
            [delta(
                1_000,
                "sign",
                Some("None"),
                Some("Some(\"TurnAround\")")
            )],
            "a window of one pass shows that pass's changes"
        );
    }

    #[test]
    fn a_field_the_windows_first_pass_stops_showing_is_gone_at_that_pass() {
        for (to_ms, passes) in [(1_000, &[1_000][..]), (2_000, &[1_000, 2_000][..])] {
            let _collecting = super::collect();
            super::deltas_in(1_000, to_ms);
            let mut follower = Follower {
                against_fixes: 0,
                sign: None,
                track: &[],
                joined: Some(7),
            };
            for &at in passes {
                super::pass(at, |out| out.owner("follow", &follower));
                follower.joined = None;
                super::deltas(|out| out.owner("follow", &follower));
            }
            assert_eq!(
                super::take(),
                [delta(1_000, "segment", Some("7"), None)],
                "window to {to_ms}"
            );
        }
    }

    #[test]
    fn a_buffer_changes_by_its_length_and_digest_and_a_field_no_longer_shown_is_gone() {
        let _collecting = super::collect();
        super::deltas_in(0, u64::MAX);
        let (short, long, edited) = ([1u16, 2], [1u16, 2, 3], [1u16, 2, 4]);
        let mut follower = Follower {
            against_fixes: 0,
            sign: None,
            track: &short,
            joined: Some(4),
        };
        let show = |at: u64, follower: &Follower<'_>| {
            super::pass(at, |out| out.owner("follow", follower));
            super::deltas(|out| out.owner("follow", follower));
        };
        show(0, &follower);
        follower.track = &long;
        show(1, &follower);
        follower.track = &edited;
        follower.joined = None;
        show(2, &follower);
        show(3, &follower);
        let entries = super::take();
        let Entry::Delta(grew) = &entries[0] else {
            panic!("{entries:?}");
        };
        let new = grew.new.as_deref().unwrap();
        assert!(grew.old.as_deref().unwrap().starts_with("2 items, digest "));
        assert!(new.starts_with("3 items, digest "), "{new}");
        let Entry::Delta(edited) = &entries[1] else {
            panic!("{entries:?}");
        };
        assert_eq!((edited.at_ms, edited.field), (2, "track"));
        assert_eq!(edited.old.as_deref(), Some(new));
        assert_ne!(edited.new.as_deref(), Some(new), "same length, other items");
        assert_eq!(entries[2], delta(2, "segment", Some("4"), None));
        assert_eq!(entries.len(), 3, "{entries:?}");
    }

    #[test]
    fn outside_the_window_no_state_is_read_and_nothing_is_formatted() {
        struct Spy<'a>(&'a core::cell::Cell<u32>);
        impl core::fmt::Debug for Spy<'_> {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                self.0.set(self.0.get() + 1);
                f.write_str("spy")
            }
        }
        let formatted = core::cell::Cell::new(0);
        let read = core::cell::Cell::new(0);
        let state = |out: &mut Fields<'_>| {
            read.set(read.get() + 1);
            out.field("spy", &Spy(&formatted));
        };
        super::pass(1_000, state);
        super::deltas(state);
        let _collecting = super::collect();
        super::pass(1_000, state);
        super::deltas(state);
        assert_eq!(read.get(), 0, "no window, no read");
        super::deltas_in(2_000, 3_000);
        for at in [1_000, 1_500, 3_001] {
            super::pass(at, state);
            super::deltas(state);
        }
        assert_eq!((read.get(), formatted.get()), (0, 0));
        super::pass(2_000, state);
        super::deltas(state);
        super::pass(2_500, state);
        super::deltas(state);
        assert_eq!(
            read.get(),
            3,
            "the baseline before the first pass, then each pass's end"
        );
        assert_eq!(
            formatted.get(),
            4,
            "the baseline digests and keeps the text; an unchanged field is digested only"
        );
        assert_eq!(super::take(), []);
    }
}
