//! The spike's benchmark on the real runtime (#496): the countdown and
//! graph workloads natively and under WAMR, which must agree on a checksum,
//! a runaway loop stopped by the instruction limit, and one production
//! script tile's draw: its time and native stack. `TEST 32` prints it on
//! the device; the host tests check the checksums.
use super::host::{Call, Job};
use super::workload::{self, Host};
use super::{STACK, guests};
use crate::compose::data::Data;
use crate::compose::solve::Rect16;
use crate::compose::tile::Forms;
use crate::screens::canvas::Canvas;
use crate::screens::scaled::Scaled;
use core::ffi::c_void;
use core::fmt::Write;

pub const COUNTDOWNS: u32 = 1000;
pub const SPARKLINES: u32 = 20;
/// The spike's runaway budget, for its microseconds to compare.
pub const RUNAWAY: u32 = 100_000;

/// What the host does for a tile: every call folds into a checksum, so the
/// work cannot be optimised away.
pub struct Checksum(pub u32);

impl Host for Checksum {
    fn sample(&mut self, i: i32) -> i32 {
        sample(i)
    }
    fn ring(&mut self, cx: i32, cy: i32, r: i32, frac: i32, color: i32) {
        self.mix((cx ^ cy ^ r ^ frac ^ color) as u32);
    }
    fn text(&mut self, digits: u32, len: i32, x: i32, y: i32, color: i32) {
        self.mix(digits ^ (len ^ x ^ y ^ color) as u32);
    }
    fn line_to(&mut self, x: i32, y: i32) {
        self.mix((x * 1000 + y) as u32);
    }
}

impl Checksum {
    fn mix(&mut self, value: u32) {
        self.0 = self.0.rotate_left(5) ^ value;
    }
}

#[inline(never)]
pub fn sample(i: i32) -> i32 {
    120 + ((i * 37) % 61) - ((i * 13) % 29)
}

fn countdown_args(k: u32) -> [u32; 2] {
    [(600_000 - k as i32 * 300) as u32, 900_000]
}

const SPARKLINE_ARGS: [u32; 3] = [600, 218, 46];

#[derive(Debug, PartialEq, Eq)]
pub struct Native {
    pub countdown_ns: u64,
    pub sparkline_us: u64,
    pub sum: u32,
}

/// Both workloads as plain Rust.
pub fn native(now: &dyn Fn() -> u64) -> Native {
    let mut host = Checksum(0);
    let start = now();
    for k in 0..COUNTDOWNS {
        let [left, total] = countdown_args(k);
        workload::countdown(&mut host, left as i32, total as i32);
    }
    let countdown = now() - start;
    let start = now();
    for _ in 0..SPARKLINES {
        let [n, width, height] = SPARKLINE_ARGS;
        workload::sparkline(&mut host, n as i32, width as i32, height as i32);
    }
    let sparkline = now() - start;
    Native {
        countdown_ns: countdown * 1000 / u64::from(COUNTDOWNS),
        sparkline_us: sparkline / u64::from(SPARKLINES),
        sum: host.0,
    }
}

#[derive(Debug)]
pub struct Interpreted {
    pub load_us: u64,
    pub instantiate_us: u64,
    pub module_pool: usize,
    pub instance_pool: usize,
    pub countdown_ns: u64,
    pub sparkline_us: u64,
    pub sum: u32,
    pub runaway_stopped: bool,
    pub runaway_us: u64,
}

/// Both workloads, and the runaway loop, in the spike's guest under WAMR.
pub fn interpreted(now: &dyn Fn() -> u64) -> Result<Interpreted, wamr::Message> {
    super::runtime(|floor| {
        let pool = wamr::pool_used();
        let start = now();
        // SAFETY: `runtime` holds the script table.
        let module = unsafe { wamr::Module::load(guests::BENCH) }?;
        let load_us = now() - start;
        let module_pool = wamr::pool_used() - pool;
        let start = now();
        // SAFETY: as above; the instance drops before the module.
        let mut instance = unsafe { wamr::Instance::new(&module, STACK, 1, floor) }?;
        let instantiate_us = now() - start;
        let instance_pool = wamr::pool_used() - pool - module_pool;
        let func = |name| {
            instance
                .func(name)
                .ok_or_else(|| wamr::Message::try_from("an export is missing").unwrap_or_default())
        };
        let (countdown, sparkline, spin) =
            (func(c"countdown")?, func(c"sparkline")?, func(c"spin")?);
        let mut sum = Checksum(0);
        let user = (&raw mut sum).cast::<c_void>();
        let start = now();
        for k in 0..COUNTDOWNS {
            // SAFETY: as above; the bench natives read `user` as a Checksum.
            unsafe { instance.call(countdown, &mut countdown_args(k), None, user) }?;
        }
        let countdown_us = now() - start;
        let start = now();
        for _ in 0..SPARKLINES {
            // SAFETY: as above.
            unsafe { instance.call(sparkline, &mut { SPARKLINE_ARGS }, None, user) }?;
        }
        let sparkline_us = now() - start;
        let start = now();
        // SAFETY: as above.
        let runaway = unsafe { instance.call(spin, &mut [1], Some(RUNAWAY), user) };
        let runaway_us = now() - start;
        Ok(Interpreted {
            load_us,
            instantiate_us,
            module_pool,
            instance_pool,
            countdown_ns: countdown_us * 1000 / u64::from(COUNTDOWNS),
            sparkline_us: sparkline_us / u64::from(SPARKLINES),
            sum: sum.0,
            runaway_stopped: runaway.is_err_and(|why| why == "instruction limit exceeded"),
            runaway_us,
        })
    })
    .unwrap_or_else(|| Err(wamr::Message::try_from("no runtime").unwrap_or_default()))
}

#[derive(Debug)]
pub struct Tile {
    pub draw_us: u64,
    /// Main stack bytes the deepest draw reached below the caller.
    pub stack: usize,
    pub shapes: usize,
    pub runs: usize,
}

const DRAWS: u32 = 100;

/// The countdown guest's first look drawn through the production host
/// API, `DRAWS` times; `depth` runs a closure and reports the stack it
/// took.
pub fn tile(
    at: &Scaled,
    data: &dyn Data,
    now: &dyn Fn() -> u64,
    depth: &dyn Fn(&mut dyn FnMut()) -> usize,
) -> Result<Tile, wamr::Message> {
    let fail = |why: &str| wamr::Message::try_from(why).unwrap_or_default();
    super::runtime(|floor| {
        // SAFETY: `runtime` holds the script table; the instance drops
        // before the module.
        let module = unsafe { wamr::Module::load(guests::COUNTDOWN) }?;
        let mut instance = unsafe { wamr::Instance::new(&module, STACK, 1, floor) }?;
        let (declare, draw) = (
            instance.func(c"forms").ok_or_else(|| fail("no forms"))?,
            instance.func(c"draw").ok_or_else(|| fail("no draw"))?,
        );
        let mut forms = Forms::new();
        Call::new(at, Job::Declare(&mut forms))
            .run(&mut instance, declare, &mut [0; 3])
            .map_err(|_| fail("forms stopped"))?;
        let form = *forms.first().ok_or_else(|| fail("no look"))?;
        let rect = Rect16 {
            left: 0,
            top: 0,
            right: form.width,
            bottom: form.height,
        };
        let mut canvas = Canvas::EMPTY;
        let mut stopped = false;
        let mut once = |canvas: &mut Canvas| {
            *canvas = Canvas::EMPTY;
            let mut args = [0, u32::from(form.width), u32::from(form.height)];
            stopped |= Call::new(at, Job::draw(rect, &form, data, canvas))
                .run(&mut instance, draw, &mut args)
                .is_err();
        };
        let stack = depth(&mut || once(&mut canvas));
        let start = now();
        for _ in 0..DRAWS {
            once(&mut canvas);
        }
        let draw_us = (now() - start) / u64::from(DRAWS);
        if stopped {
            return Err(fail("the countdown tile stopped"));
        }
        Ok(Tile {
            draw_us,
            stack,
            shapes: canvas.shapes.len(),
            runs: canvas.runs.len(),
        })
    })
    .unwrap_or_else(|| Err(fail("no runtime")))
}

/// Everything above, as `TEST 32` prints it.
pub fn report(
    at: &Scaled,
    data: &dyn Data,
    now: &dyn Fn() -> u64,
    depth: &dyn Fn(&mut dyn FnMut()) -> usize,
    out: &mut dyn Write,
) {
    let native = native(now);
    let _ = write!(
        out,
        "native countdown_ns={} sparkline_us={} sum={:08x} | ",
        native.countdown_ns, native.sparkline_us, native.sum
    );
    match interpreted(now) {
        Ok(wamr) => {
            let _ = write!(
                out,
                "wamr load_us={} instantiate_us={} module_pool={} instance_pool={} countdown_ns={} sparkline_us={} sum={:08x} same={} runaway_stopped={} limit={} in_us={} | ",
                wamr.load_us,
                wamr.instantiate_us,
                wamr.module_pool,
                wamr.instance_pool,
                wamr.countdown_ns,
                wamr.sparkline_us,
                wamr.sum,
                wamr.sum == native.sum,
                wamr.runaway_stopped,
                RUNAWAY,
                wamr.runaway_us
            );
        }
        Err(why) => {
            let _ = write!(out, "wamr failed: {why} | ");
        }
    }
    match tile(at, data, now, depth) {
        Ok(tile) => {
            let _ = write!(
                out,
                "tile draw_us={} stack={} shapes={} runs={} limit={} pool_used={}",
                tile.draw_us,
                tile.stack,
                tile.shapes,
                tile.runs,
                super::LIMIT,
                wamr::pool_used()
            );
        }
        Err(why) => {
            let _ = write!(out, "tile failed: {why}");
        }
    }
}
