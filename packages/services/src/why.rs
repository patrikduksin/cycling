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
    ($tag:literal $(, $name:ident $(= $value:expr)?)* $(,)?) => {
        if false {
            let _ = ($(&$crate::__why_operand!($name $(= $value)?),)*);
        }
    };
}

#[cfg(not(target_os = "none"))]
#[macro_export]
macro_rules! why {
    ($tag:literal $(, $name:ident $(= $value:expr)?)* $(,)?) => {
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

    #[derive(Default)]
    struct Collector {
        at_ms: u64,
        pass: u64,
        entries: Vec<Why>,
        last: BTreeMap<&'static str, LastNote>,
    }

    struct LastNote {
        entry: usize,
        noted_pass: u64,
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

    /// A pass begins at `now_ms`: what is noted until the next one is
    /// stamped with it.
    pub fn pass(now_ms: u64) {
        if !collecting() {
            return;
        }
        COLLECTOR.with_borrow_mut(|collector| {
            collector.at_ms = now_ms;
            collector.pass += 1;
        });
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
                && collector.entries[last.entry].operands == operands
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
            collector.entries.push(Why {
                at_ms,
                tag,
                operands,
            });
        });
    }

    /// The entries so far, in the order they were noted; collecting goes on
    /// from nothing.
    pub fn take() -> Vec<Why> {
        COLLECTOR.with_borrow_mut(|collector| {
            collector.last.clear();
            core::mem::take(&mut collector.entries)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Why;
    use std::string::{String, ToString};
    use std::vec::Vec;

    fn entry(at_ms: u64, tag: &'static str, operands: &[(&'static str, &str)]) -> Why {
        Why {
            at_ms,
            tag,
            operands: operands
                .iter()
                .map(|(name, value)| (*name, value.to_string()))
                .collect(),
        }
    }

    #[test]
    fn a_note_names_its_operands_by_identifier_or_by_name() {
        let _collecting = super::collect();
        super::pass(1_000);
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
        super::pass(5);
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
            super::pass(at);
            why!("toast.none", page);
            why!("toast.none", page);
        };
        for at in [0, 100, 200, 300] {
            page(at, "home");
        }
        page(400, "ride");
        page(500, "home");
        super::pass(600);
        super::pass(700);
        page(800, "home");
        let noted: Vec<(u64, String)> = super::take()
            .into_iter()
            .map(|why| (why.at_ms, why.operands[0].1.clone()))
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
}
