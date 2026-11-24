//! Script tiles: WebAssembly modules that WAMR's fast interpreter runs, so a
//! tile can arrive without a reflash (#496, Runtime tiles).
//!
//! A module is installed into one of [`SLOTS`] slots with [`install`], and
//! a config names the slot as a [`Script`](super::tiles::script::Script)
//! tile's setting. Accept makes one instance for each slot the config
//! names, at most [`INSTANCES`], asks each for its looks on the panel, and
//! draws every look under [`LIMIT`] with no readings, then with every
//! reading at its lowest and at its highest. A module that traps or runs
//! past the limit is refused with the reason. Instances live until the next
//! accept; a frame only calls into them.
//!
//! The guest's side of the host API is `apps/vana/guests/vana.rs`.

pub mod bench;
pub mod guests;
mod host;
#[cfg(test)]
mod tests;
pub mod workload;

use super::data::{Data, Effort, Metric, Nothing};
use super::reject::Why;
use super::solve::{Placed, Rect16};
use super::tile::Forms;
use crate::screens::canvas::Canvas;
use crate::screens::scaled::Scaled;
use core::cell::UnsafeCell;
use core::fmt::{self, Write as _};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use host::{Call, Job};

/// One instance's linear memory, which the device gives [`start`].
pub use wamr::Memory;

/// Installed modules.
pub const SLOTS: usize = 4;
/// Live instances: each takes a 64 KB page of PSRAM.
pub const INSTANCES: usize = 2;
/// The longest module [`install`] takes.
pub const MODULE_BYTES: usize = 4 * 1024;
/// Script tiles one tree shows.
pub const TILES_A_TREE: usize = 2;
/// The runtime's pool, for everything but linear memory: [`SLOTS`] modules
/// of [`MODULE_BYTES`] and [`INSTANCES`] instances with their stacks.
pub const POOL: usize = 64 * 1024;
/// Each instance's operand stack, in the pool.
const STACK: u32 = 4096;

/// Instructions the fast interpreter runs a millisecond on the C606: the
/// spike's runaway loop ran out of 100,000 in 20 ms (#496); `TEST 32`
/// measures it again.
const PER_MS: u32 = 100_000 / 20;
/// Script tiles' share of an 11 ms frame (#496).
const SHARE_US: u32 = 2_000;
/// A frame draws a screen and at most one layer over it.
const CALLS: u32 = 2 * TILES_A_TREE as u32;
/// The instructions one script call may run: its share of the frame,
/// 500 µs, at the C606's rate.
pub const LIMIT: u32 = PER_MS * SHARE_US / 1_000 / CALLS;

/// Why [`install`] refused a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    Slot(u8),
    Long(usize),
    /// The device never gave the runtime its memory.
    Runtime,
    /// WAMR's loader refused it: its words.
    Invalid(wamr::Message),
    /// It imports something the host does not offer: the import's name.
    Import(wamr::Message),
    /// It does not export this function.
    Export(&'static str),
    /// Instantiating it would run this, outside any limit.
    Starts(&'static str),
}

/// Why accept refused a config's script tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// No module is installed in the slot.
    Missing,
    /// Its instance did not fit: it asks for more than one page of linear
    /// memory, or the runtime has no room left.
    Memory,
    /// `forms` or `draw` is missing or takes the wrong arguments.
    Exports,
    /// It declared no look, or a look larger than the one before it.
    Looks,
    /// `forms` (no look) or `draw` of a look stopped.
    Stopped { look: Option<u8>, trap: Trap },
}

/// What stopped a script call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trap {
    /// It ran past [`LIMIT`].
    Limit,
    /// Its call reached the native stack's floor.
    Stack,
    /// It drew outside its box.
    Outside,
    /// It drew more shapes or text runs than its look declared.
    Overdraw,
    /// It called the host out of turn or with a setting the host lacks.
    Host,
    /// The module itself trapped: unreachable, a bad memory access, a
    /// division by zero.
    Guest,
}

/// A module the slot table holds, and its instance while a config names it.
struct Live {
    slot: u8,
    instance: wamr::Instance,
    draw: wamr::Func,
    forms: Forms,
}

struct Table {
    started: bool,
    floor: Option<NonNull<u8>>,
    live: [Option<Live>; INSTANCES],
    modules: [Option<wamr::Module>; SLOTS],
}

struct Shared {
    busy: AtomicBool,
    table: UnsafeCell<Table>,
}

// SAFETY: `with` hands out the table to one caller at a time, and WAMR is
// reached only through it.
unsafe impl Sync for Shared {}

static SHARED: Shared = Shared {
    busy: AtomicBool::new(false),
    table: UnsafeCell::new(Table {
        started: false,
        floor: None,
        live: [const { None }; INSTANCES],
        modules: [const { None }; SLOTS],
    }),
};

struct Held;

impl Drop for Held {
    fn drop(&mut self) {
        SHARED.busy.store(false, Ordering::Release);
    }
}

/// The table, and WAMR with it, for one caller at a time.
fn with<R>(f: impl FnOnce(&mut Table) -> R) -> R {
    while SHARED
        .busy
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let _held = Held;
    // SAFETY: `busy` admits one caller, and natives never re-enter `with`.
    f(unsafe { &mut *SHARED.table.get() })
}

/// Start the runtime on the device's memory: `pool` ([`POOL`] bytes) and
/// one page for each instance. A script call that would reach below
/// `floor` on the native stack stops instead. False if the runtime did not
/// start.
///
/// # Safety
///
/// Call once, before anything else here; the runtime owns both from here
/// on, and `floor` lies at least as far above the stack's end as a host
/// call reaches below WAMR's check.
pub unsafe fn start(
    pool: &'static mut [u8],
    memories: &'static mut [Memory; INSTANCES],
    floor: Option<NonNull<u8>>,
) -> bool {
    with(|table| {
        // SAFETY: as the caller promised.
        unsafe { table.start(pool, memories, floor) }
    })
}

impl Table {
    /// # Safety
    ///
    /// As [`start`]; the table is held.
    unsafe fn start(
        &mut self,
        pool: &'static mut [u8],
        memories: &'static mut [wamr::Memory],
        floor: Option<NonNull<u8>>,
    ) -> bool {
        // SAFETY: the table is held, so nothing else is in WAMR.
        self.started = !self.started && unsafe { wamr::init(pool, memories) && host::register() };
        self.floor = floor;
        self.started
    }

    /// The simulator and the tests run on the host's own memory.
    #[cfg(not(target_os = "none"))]
    fn running(&mut self) -> bool {
        if !self.started {
            static mut POOL_BYTES: [u8; POOL] = [0; POOL];
            static mut MEMORIES: [wamr::Memory; INSTANCES] =
                [const { wamr::Memory([0; wamr::PAGE]) }; INSTANCES];
            // SAFETY: the table is held and has not started, so nothing
            // else has touched either static.
            let started = unsafe {
                self.start(
                    core::slice::from_raw_parts_mut((&raw mut POOL_BYTES).cast(), POOL),
                    core::slice::from_raw_parts_mut((&raw mut MEMORIES).cast(), INSTANCES),
                    None,
                )
            };
            assert!(started, "WAMR starts on the host");
        }
        true
    }

    #[cfg(target_os = "none")]
    fn running(&mut self) -> bool {
        self.started
    }

    fn live(&mut self, slot: u8) -> Option<&mut Live> {
        self.live
            .iter_mut()
            .flatten()
            .find(|live| live.slot == slot)
    }

    fn drop_instances(&mut self) {
        self.live = [const { None }; INSTANCES];
    }
}

/// Validate `wasm` and keep it in `slot`; the bytes may go once this
/// returns. A refusal leaves the slot as it was. A config showing the slot
/// loses its instance: accept it again to show the new module.
pub fn install(slot: u8, wasm: &[u8]) -> Result<(), Refused> {
    if usize::from(slot) >= SLOTS {
        return Err(Refused::Slot(slot));
    }
    if wasm.len() > MODULE_BYTES {
        return Err(Refused::Long(wasm.len()));
    }
    with(|table| {
        if !table.running() {
            return Err(Refused::Runtime);
        }
        // SAFETY: the table is held.
        let module = unsafe { wamr::Module::load(wasm) }.map_err(Refused::Invalid)?;
        let mut index = 0;
        while let Some(import) = module.import(index) {
            if !(import.function && import.linked && import.from == "vana") {
                let mut name = wamr::Message::new();
                let _ = write!(name, "{}.{}", import.from, import.name);
                return Err(Refused::Import(name));
            }
            index += 1;
        }
        for export in ["forms", "draw"] {
            if !module.exports(export) {
                return Err(Refused::Export(export));
            }
        }
        if let Some(entry) = module.entry() {
            return Err(Refused::Starts(entry));
        }
        for live in &mut table.live {
            if live.as_ref().is_some_and(|live| live.slot == slot) {
                *live = None;
            }
        }
        table.modules[usize::from(slot)] = Some(module);
        Ok(())
    })
}

/// Make an instance for each of `slots` on panel `at`, in place of every
/// instance before; record each one's looks and draw each look with each
/// of [`TRIALS`], under [`LIMIT`]. On a refusal no instance is left.
pub(crate) fn ready(slots: &[u8], at: &Scaled) -> Result<(), Why> {
    with(|table| {
        table.drop_instances();
        for (index, &slot) in slots.iter().enumerate() {
            match table.make(slot, at) {
                Ok(live) => table.live[index] = Some(live),
                Err(fault) => {
                    table.drop_instances();
                    return Err(Why::Script { slot, fault });
                }
            }
        }
        Ok(())
    })
}

/// Drop every instance: the config that named them was refused.
pub(crate) fn clear() {
    with(Table::drop_instances);
}

impl Table {
    fn make(&mut self, slot: u8, at: &Scaled) -> Result<Live, Fault> {
        let module = self.modules[usize::from(slot)]
            .as_ref()
            .ok_or(Fault::Missing)?;
        // SAFETY: the table is held, and `install` drops a slot's instance
        // before it replaces the module.
        let mut instance = unsafe { wamr::Instance::new(module, STACK, 1, self.floor) }
            .map_err(|_| Fault::Memory)?;
        let shaped = |name, params| {
            instance
                .func(name)
                .filter(|func| func.params == params && func.results == 0)
        };
        let (Some(declare), Some(draw)) = (shaped(c"forms", 0), shaped(c"draw", 3)) else {
            return Err(Fault::Exports);
        };
        let mut forms = Forms::new();
        let mut call = Call::new(at, Job::Declare(&mut forms));
        call.run(&mut instance, declare, &mut [0; 3])
            .map_err(|trap| Fault::Stopped { look: None, trap })?;
        if forms.is_empty()
            || !forms
                .windows(2)
                .all(|pair| pair[1].width <= pair[0].width && pair[1].height <= pair[0].height)
        {
            return Err(Fault::Looks);
        }
        for (look, form) in forms.iter().enumerate() {
            let rect = Rect16 {
                left: 0,
                top: 0,
                right: form.width,
                bottom: form.height,
            };
            for data in TRIALS {
                let mut canvas = Canvas::EMPTY;
                let mut call = Call::new(at, Job::draw(rect, form, data, &mut canvas));
                let mut args = [look as u32, u32::from(form.width), u32::from(form.height)];
                call.run(&mut instance, draw, &mut args)
                    .map_err(|trap| Fault::Stopped {
                        look: Some(look as u8),
                        trap,
                    })?;
            }
        }
        Ok(Live {
            slot,
            instance,
            draw,
            forms,
        })
    }
}

/// The readings accept draws each look with.
const TRIALS: [&dyn Data; 3] = [&Nothing, &End::Low, &End::High];

/// Every metric at one end of what it shows, every zone at one end of its
/// efforts', through the whole window.
enum End {
    Low,
    High,
}

impl Data for End {
    fn metric(&self, metric: Metric) -> Option<i32> {
        let (low, high) = metric.ends();
        Some(match self {
            Self::Low => low,
            Self::High => high,
        })
    }

    fn zone(&self, effort: Effort) -> Option<u8> {
        Some(match self {
            Self::Low => 1,
            Self::High => effort.zones(),
        })
    }

    fn sample(&self, metric: Metric, _: u16) -> Option<i32> {
        self.metric(metric)
    }
}

/// The looks accept recorded for `slot`; none without an instance.
pub(crate) fn forms(slot: u8) -> Forms {
    with(|table| {
        table
            .live(slot)
            .map(|live| live.forms.clone())
            .unwrap_or_default()
    })
}

/// Draw `slot`'s look `placed.form` into `placed.rect`. A call that stops
/// leaves nothing on `out`, and the frame goes on without the tile.
pub(crate) fn draw(slot: u8, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
    with(|table| {
        let Some(live) = table.live(slot) else {
            return;
        };
        let Some(form) = live.forms.get(usize::from(placed.form)) else {
            return;
        };
        let (shapes, runs) = (out.shapes.len(), out.runs.len());
        let rect = placed.rect;
        let mut args = [
            u32::from(placed.form),
            u32::from(rect.width()),
            u32::from(rect.height()),
        ];
        let mut call = Call::new(at, Job::draw(rect, form, data, out));
        if call.run(&mut live.instance, live.draw, &mut args).is_err() {
            out.shapes.truncate(shapes);
            out.runs.truncate(runs);
        }
    });
}

/// The runtime, started, for the benchmark: none on a device that never
/// gave it memory.
pub(crate) fn runtime<R>(f: impl FnOnce(Option<NonNull<u8>>) -> R) -> Option<R> {
    with(|table| table.running().then(|| f(table.floor)))
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Slot(slot) => write!(f, "there is no script slot {slot}"),
            Self::Long(length) => {
                write!(f, "{length} bytes is longer than a script ({MODULE_BYTES})")
            }
            Self::Runtime => f.write_str("this device runs no scripts"),
            Self::Invalid(why) => write!(f, "not a valid module: {why}"),
            Self::Import(name) => write!(f, "imports {name}, which VANA does not offer"),
            Self::Export(name) => write!(f, "exports no function {name}"),
            Self::Starts(entry) => write!(f, "runs {entry} when instantiated"),
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Missing => f.write_str("no script is installed there"),
            Self::Memory => f.write_str("no room for its instance (a script gets one 64 KB page)"),
            Self::Exports => f.write_str("it exports no forms() and draw(look, width, height)"),
            Self::Looks => {
                f.write_str("it declares no look, or a look larger than the one before it")
            }
            Self::Stopped { look: None, trap } => write!(f, "declaring its looks, it {trap}"),
            Self::Stopped {
                look: Some(look),
                trap,
            } => write!(f, "drawing look {look}, it {trap}"),
        }
    }
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limit => write!(f, "ran past its {LIMIT}-instruction limit"),
            Self::Stack => f.write_str("ran out of native stack"),
            Self::Outside => f.write_str("drew outside its box"),
            Self::Overdraw => f.write_str("drew more than its look declares"),
            Self::Host => f.write_str("asked the host for something it lacks"),
            Self::Guest => f.write_str("trapped"),
        }
    }
}
