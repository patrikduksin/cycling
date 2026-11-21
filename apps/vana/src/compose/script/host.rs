//! What a script may call: module `vana`, and the benchmark's `bench`.
//! Every `vana` native finds the call it serves through WAMR's user data
//! and stops the guest, rather than draw, on anything outside its look.
//!
//! WAMR counts one instruction for a host call however much the host does,
//! so each native also spends from the call's own [`LIMIT`] of host work,
//! priced in instructions' worth, and stops the guest once it runs out.
use super::bench::Checksum;
use super::{LIMIT, Trap};
use crate::compose::data::{Data, Effort, Metric};
use crate::compose::solve::Rect16;
use crate::compose::tile::{Form, Forms};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Align, Rect, Ring, TURN};
use crate::screens::scaled::Scaled;
use core::ffi::c_void;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};

/// `metric` without a reading.
const NONE: i32 = i32::MIN;
/// The longest window `series` draws.
const MAX_SECONDS: i32 = 600;
/// The most of a guest's text a native reads: what one run holds
/// ([`on_baseline`](crate::screens::canvas::on_baseline) takes a `Line<24>`).
pub(super) const LONGEST: usize = 24;
/// What a host call costs before its own work, in instructions: the spike's
/// graph made 1,400 host calls at about 2.75 µs each on the C606, 14
/// instructions' worth (#496).
const CALL: u32 = 16;
/// What `text` costs a character: `Line::fit` measures each shorter prefix
/// of a text too wide for its box (a guess; `TEST 32` measures it).
const FIT: u32 = 12;
/// What `ring` costs past the call: its sweep and its reach (a guess;
/// `TEST 32` measures it).
const RING: u32 = 32;
const FACES: [FaceRole; 6] = [
    FaceRole::Caption,
    FaceRole::Body,
    FaceRole::Title,
    FaceRole::Value,
    FaceRole::Large,
    FaceRole::Hero,
];

pub(super) enum Job<'a> {
    /// `forms`: the looks it declares.
    Declare(&'a mut Forms),
    /// `draw`: into `rect`, at most `shapes` shapes and `runs` runs more.
    Draw {
        rect: Rect16,
        shapes: u8,
        runs: u8,
        data: &'a dyn Data,
        out: &'a mut Canvas,
    },
}

impl<'a> Job<'a> {
    pub(super) fn draw(rect: Rect16, form: &Form, data: &'a dyn Data, out: &'a mut Canvas) -> Self {
        Self::Draw {
            rect,
            shapes: form.shapes,
            runs: form.runs,
            data,
            out,
        }
    }
}

pub(super) struct Call<'a> {
    at: &'a Scaled,
    job: Job<'a>,
    /// What a native stopped the guest for.
    stop: Option<Trap>,
    /// The host work the call's natives may still do, in instructions.
    work: u32,
    /// A native's panic, raised again once the call is out of WAMR.
    #[cfg(any(test, feature = "invariants"))]
    panic: Option<std::boxed::Box<dyn core::any::Any + Send>>,
}

impl<'a> Call<'a> {
    pub(super) fn new(at: &'a Scaled, job: Job<'a>) -> Self {
        Self {
            at,
            job,
            stop: None,
            work: LIMIT,
            #[cfg(any(test, feature = "invariants"))]
            panic: None,
        }
    }

    /// Run `func` under [`LIMIT`](super::LIMIT).
    pub(super) fn run(
        &mut self,
        instance: &mut wamr::Instance,
        func: wamr::Func,
        args: &mut [u32],
    ) -> Result<(), Trap> {
        let user = (self as *mut Self).cast::<c_void>();
        // SAFETY: the script table is held; `func` is the instance's own,
        // and the natives registered for it read `user` as this `Call`.
        let ran = unsafe { instance.call(func, args, Some(LIMIT), user) };
        #[cfg(any(test, feature = "invariants"))]
        if let Some(panic) = self.panic.take() {
            std::panic::resume_unwind(panic);
        }
        ran.map_err(|message| {
            self.stop.unwrap_or(match message.as_str() {
                "instruction limit exceeded" => Trap::Limit,
                "native stack overflow" => Trap::Stack,
                _ => Trap::Guest,
            })
        })
    }

    /// Take `cost` instructions' worth of host work from the call.
    fn charge(&mut self, cost: u32) -> Result<(), Trap> {
        self.work = self.work.checked_sub(cost).ok_or(Trap::Limit)?;
        Ok(())
    }

    /// The draw job, with `shapes` shapes and `runs` runs taken from its
    /// look's budget.
    fn spend(&mut self, shapes: u8, runs: u8) -> Result<(Rect16, &dyn Data, &mut Canvas), Trap> {
        let Job::Draw {
            rect,
            shapes: shapes_left,
            runs: runs_left,
            data,
            out,
        } = &mut self.job
        else {
            return Err(Trap::Host);
        };
        *shapes_left = shapes_left.checked_sub(shapes).ok_or(Trap::Overdraw)?;
        *runs_left = runs_left.checked_sub(runs).ok_or(Trap::Overdraw)?;
        Ok((*rect, *data, out))
    }

    fn data(&self) -> Result<&dyn Data, Trap> {
        match &self.job {
            Job::Draw { data, .. } => Ok(*data),
            // Looks are fixed at accept, so declaring them may not read the ride.
            Job::Declare(_) => Err(Trap::Host),
        }
    }
}

/// The call `env` serves, charged [`CALL`], and the native's verdict
/// applied to it.
///
/// # Safety
///
/// `env` is the environment WAMR passed a native registered by [`register`].
unsafe fn serve<R: Default>(
    env: *mut c_void,
    native: impl FnOnce(&mut Call<'_>) -> Result<R, Trap>,
) -> R {
    // SAFETY: every call into a module that imports `vana` passes its
    // `Call` as user data; only the `bench` guest gets a `Checksum`.
    let Some(call) = (unsafe { wamr::user_data(env).cast::<Call<'_>>().as_mut() }) else {
        return R::default();
    };
    let native = |call: &mut Call<'_>| {
        call.charge(CALL)?;
        native(call)
    };
    match guarded(call, native) {
        Ok(value) => value,
        Err(trap) => {
            call.stop = Some(trap);
            // SAFETY: as above.
            unsafe { wamr::trap(env, c"stopped by the host") };
            R::default()
        }
    }
}

/// `native`'s verdict. Natives are `extern "C"`, so a panic must not
/// unwind out of one: where the host can catch it, it stops the guest and
/// [`Call::run`] raises it again.
#[cfg(any(test, feature = "invariants"))]
fn guarded<R>(
    call: &mut Call<'_>,
    native: impl FnOnce(&mut Call<'_>) -> Result<R, Trap>,
) -> Result<R, Trap> {
    std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| native(call))).unwrap_or_else(
        |panic| {
            call.panic = Some(panic);
            Err(Trap::Host)
        },
    )
}

/// The device's panics halt it wherever they start.
#[cfg(not(any(test, feature = "invariants")))]
fn guarded<R>(
    call: &mut Call<'_>,
    native: impl FnOnce(&mut Call<'_>) -> Result<R, Trap>,
) -> Result<R, Trap> {
    native(call)
}

fn face(call: &Call<'_>, face: i32) -> Result<&'static Face, Trap> {
    let role = usize::try_from(face)
        .ok()
        .and_then(|index| FACES.get(index))
        .ok_or(Trap::Host)?;
    Ok(call.at.face(*role))
}

/// The first [`LONGEST`] of the `len` bytes at `text`; WAMR passes a null
/// `text` for an empty one at the end of memory.
///
/// # Safety
///
/// `len` bytes at `text` are readable while the result lives, unless `len`
/// is zero.
pub(super) unsafe fn guest_text<'t>(text: *const u8, len: u32) -> &'t [u8] {
    if len == 0 {
        return &[];
    }
    let len = (len as usize).min(LONGEST);
    // SAFETY: as the caller promised.
    unsafe { core::slice::from_raw_parts(text, len) }
}

/// `(left, top, width, height)` from the box's corner, as a rect on the
/// panel, if it lies inside the box.
fn inside(rect: Rect16, left: i32, top: i32, width: i32, height: i32) -> Result<Rect, Trap> {
    let fits = |start: i32, length: i32, room: u16| {
        start >= 0
            && length >= 0
            && start
                .checked_add(length)
                .is_some_and(|end| end <= i32::from(room))
    };
    if !(fits(left, width, rect.width()) && fits(top, height, rect.height())) {
        return Err(Trap::Outside);
    }
    let (left, top) = (
        usize::from(rect.left) + left as usize,
        usize::from(rect.top) + top as usize,
    );
    Ok(Rect {
        left,
        top,
        right: left + width as usize,
        bottom: top + height as usize,
    })
}

fn within(outer: Rect16, inner: Rect) -> bool {
    let outer = outer.rect();
    inner.left >= outer.left
        && inner.top >= outer.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}

extern "C" fn form(env: *mut c_void, width: i32, height: i32, shapes: i32, runs: i32) {
    // SAFETY: WAMR passed `env` to this native.
    unsafe {
        serve(env, |call| {
            let Job::Declare(forms) = &mut call.job else {
                return Err(Trap::Host);
            };
            let form = Form {
                width: u16::try_from(width).map_err(|_| Trap::Host)?,
                height: u16::try_from(height).map_err(|_| Trap::Host)?,
                shapes: u8::try_from(shapes).map_err(|_| Trap::Host)?,
                runs: u8::try_from(runs).map_err(|_| Trap::Host)?,
            };
            forms.push(form).map_err(|_| Trap::Host)
        })
    }
}

extern "C" fn unit(env: *mut c_void, n: i32) -> i32 {
    // SAFETY: WAMR passed `env` to this native.
    unsafe {
        serve(env, |call| match usize::try_from(n) {
            Ok(n) if n <= 1_000 => Ok(call.at.unit(n) as i32),
            _ => Err(Trap::Host),
        })
    }
}

extern "C" fn text_width(env: *mut c_void, role: i32, text: *const u8, len: u32) -> i32 {
    // SAFETY: WAMR passed `env` to this native.
    unsafe {
        serve(env, |call| {
            // WAMR checked that `len` bytes at `text` lie in the guest's
            // memory before the native ran (`*~`).
            let text = guest_text(text, len);
            call.charge(text.len() as u32)?;
            Ok(face(call, role)?.fit_width(text) as i32)
        })
    }
}

extern "C" fn text_height(env: *mut c_void, role: i32) -> i32 {
    // SAFETY: WAMR passed `env` to this native.
    unsafe { serve(env, |call| Ok(face(call, role)?.fit_height() as i32)) }
}

fn metric_of(id: i32) -> Result<Metric, Trap> {
    u8::try_from(id)
        .ok()
        .and_then(Metric::from_id)
        .ok_or(Trap::Host)
}

extern "C" fn metric(env: *mut c_void, id: i32) -> i32 {
    // SAFETY: WAMR passed `env` to this native.
    unsafe {
        serve(env, |call| {
            Ok(call.data()?.metric(metric_of(id)?).unwrap_or(NONE))
        })
    }
}

extern "C" fn zone(env: *mut c_void, effort: i32) -> i32 {
    // SAFETY: WAMR passed `env` to this native.
    unsafe {
        serve(env, |call| {
            let effort = u8::try_from(effort)
                .ok()
                .and_then(Effort::from_id)
                .ok_or(Trap::Host)?;
            Ok(call.data()?.zone(effort).map_or(0, i32::from))
        })
    }
}

extern "C" fn bar(env: *mut c_void, left: i32, top: i32, width: i32, height: i32, color: i32) {
    // SAFETY: WAMR passed `env` to this native.
    unsafe {
        serve(env, |call| {
            let (rect, _, out) = call.spend(1, 0)?;
            let rect = inside(rect, left, top, width, height)?;
            out.shape(Shape::Bar {
                rect,
                radius: 0,
                color: color as u16,
            });
            Ok(())
        })
    }
}

extern "C" fn ring(
    env: *mut c_void,
    cx: i32,
    cy: i32,
    radius: i32,
    width: i32,
    frac_q16: i32,
    color: i32,
) {
    // SAFETY: WAMR passed `env` to this native.
    unsafe {
        serve(env, |call| {
            call.charge(RING)?;
            let (rect, _, out) = call.spend(1, 0)?;
            if !(0..=i32::from(u16::MAX)).contains(&cx.max(cy).max(radius).max(width))
                || cx.min(cy).min(radius).min(width) < 0
            {
                return Err(Trap::Outside);
            }
            let sweep = (i64::from(frac_q16.clamp(0, 1 << 16)) * i64::from(TURN)) >> 16;
            let ring = Ring {
                cx: 4 * (i32::from(rect.left) + cx),
                cy: 4 * (i32::from(rect.top) + cy),
                radius: 4 * radius,
                width: 4 * width,
                start: 0,
                sweep: sweep as u16,
            };
            let reach = ring.reach();
            if 2 * radius < width
                || 4 * cx < reach
                || 4 * cy < reach
                || !within(rect, ring.bounds())
            {
                return Err(Trap::Outside);
            }
            out.shape(Shape::ring(ring, color as u16));
            Ok(())
        })
    }
}

extern "C" fn text(
    env: *mut c_void,
    role: i32,
    text: *const u8,
    len: u32,
    base: i32,
    align: i32,
    color: i32,
) {
    // SAFETY: WAMR passed `env` to this native.
    unsafe {
        serve(env, |call| {
            let face = face(call, role)?;
            let align = match align {
                0 => Align::Left,
                1 => Align::Center,
                2 => Align::Right,
                _ => return Err(Trap::Host),
            };
            // WAMR checked that `len` bytes at `text` lie in the guest's
            // memory before the native ran (`*~`).
            let text = guest_text(text, len);
            call.charge(FIT * text.len() as u32)?;
            let (rect, _, out) = call.spend(0, 1)?;
            let height = face.fit_height() as i32;
            if base < height || base > i32::from(rect.height()) {
                return Err(Trap::Outside);
            }
            let x = match align {
                Align::Left => usize::from(rect.left),
                Align::Center => usize::from(rect.left + rect.width() / 2),
                // Text in a box narrower than SPILL fits as no text.
                Align::Right => usize::from(rect.right)
                    .saturating_sub(SPILL)
                    .max(usize::from(rect.left)),
            };
            let base = usize::from(rect.top) + base as usize;
            out.fit(
                &[face],
                text,
                usize::from(rect.width()),
                x,
                base,
                align,
                color as u16,
            );
            Ok(())
        })
    }
}

#[allow(clippy::too_many_arguments)]
extern "C" fn series(
    env: *mut c_void,
    id: i32,
    seconds: i32,
    bars: i32,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    color: i32,
) {
    // SAFETY: WAMR passed `env` to this native.
    unsafe {
        serve(env, |call| {
            let metric = metric_of(id)?;
            let bars_u8 = u8::try_from(bars).map_err(|_| Trap::Host)?;
            if !(1..=MAX_SECONDS).contains(&seconds) || bars == 0 || bars > seconds {
                return Err(Trap::Host);
            }
            // Each second is read twice, at about half an instruction each (a
            // guess; `TEST 32` measures it).
            call.charge(seconds as u32 + u32::from(bars_u8))?;
            let (rect, data, out) = call.spend(bars_u8, 0)?;
            let rect = inside(rect, left, top, width, height)?;
            if rect.right - rect.left < usize::from(bars_u8) || rect.bottom == rect.top {
                return Err(Trap::Outside);
            }
            draw_series(
                data,
                metric,
                seconds as u16,
                bars_u8,
                rect,
                color as u16,
                out,
            );
            Ok(())
        })
    }
}

/// The last `seconds` of `metric`, oldest on the left, as `bars` bars from
/// the window's lowest reading (one pixel) to its highest (`rect`'s top),
/// each at least a column wide: `rect` has a column for each bar and a row.
/// A bar with no reading in its stretch is left out.
fn draw_series(
    data: &dyn Data,
    metric: Metric,
    seconds: u16,
    bars: u8,
    rect: Rect,
    color: u16,
    out: &mut Canvas,
) {
    let reading = |at: u16| data.sample(metric, seconds - 1 - at);
    let Some((low, high)) = (0..seconds).filter_map(reading).fold(None, |range, value| {
        let (low, high) = range.unwrap_or((value, value));
        Some((low.min(value), high.max(value)))
    }) else {
        return;
    };
    let span = i64::from(high) - i64::from(low);
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let (bars, seconds) = (usize::from(bars), usize::from(seconds));
    let gap = usize::from(width / bars >= 3);
    for bar in 0..bars {
        let stretch = seconds * bar / bars..seconds * (bar + 1) / bars;
        let (sum, count) = stretch
            .filter_map(|at| reading(at as u16))
            .fold((0i64, 0i64), |(sum, count), value| {
                (sum + i64::from(value), count + 1)
            });
        if count == 0 {
            continue;
        }
        let rise = if span == 0 {
            height - 1
        } else {
            ((sum / count - i64::from(low)) * (height as i64 - 1) / span) as usize
        };
        let (left, right) = (
            rect.left + width * bar / bars,
            rect.left + width * (bar + 1) / bars - gap,
        );
        out.shape(Shape::Bar {
            rect: Rect {
                left,
                top: rect.bottom - 1 - rise,
                right,
                bottom: rect.bottom,
            },
            radius: 0,
            color,
        });
    }
}

/// The spike's host for its benchmark guest: every call folds into the
/// checksum the call was given.
fn checksum<'a>(env: *mut c_void) -> Option<&'a mut Checksum> {
    // SAFETY: the benchmark passes its `Checksum` as user data.
    unsafe { wamr::user_data(env).cast::<Checksum>().as_mut() }
}

extern "C" fn bench_sample(_: *mut c_void, i: i32) -> i32 {
    super::bench::sample(i)
}

extern "C" fn bench_ring(env: *mut c_void, cx: i32, cy: i32, r: i32, frac: i32, color: i32) {
    if let Some(sum) = checksum(env) {
        super::workload::Host::ring(sum, cx, cy, r, frac, color);
    }
}

extern "C" fn bench_text(env: *mut c_void, digits: i32, len: i32, x: i32, y: i32, color: i32) {
    if let Some(sum) = checksum(env) {
        super::workload::Host::text(sum, digits as u32, len, x, y, color);
    }
}

extern "C" fn bench_line_to(env: *mut c_void, x: i32, y: i32) {
    if let Some(sum) = checksum(env) {
        super::workload::Host::line_to(sum, x, y);
    }
}

/// Offer the natives to modules, once, before any loads.
///
/// # Safety
///
/// The script table is held and WAMR has started.
pub(super) unsafe fn register() -> bool {
    use wamr::{Native, Natives};
    static VANA: Natives<10> = Natives::new([
        Native::new(c"form", form as *const (), c"(iiii)"),
        Native::new(c"unit", unit as *const (), c"(i)i"),
        Native::new(c"text_width", text_width as *const (), c"(i*~)i"),
        Native::new(c"text_height", text_height as *const (), c"(i)i"),
        Native::new(c"metric", metric as *const (), c"(i)i"),
        Native::new(c"zone", zone as *const (), c"(i)i"),
        Native::new(c"bar", bar as *const (), c"(iiiii)"),
        Native::new(c"ring", ring as *const (), c"(iiiiii)"),
        Native::new(c"text", text as *const (), c"(i*~iii)"),
        Native::new(c"series", series as *const (), c"(iiiiiiii)"),
    ]);
    static BENCH: Natives<4> = Natives::new([
        Native::new(c"sample", bench_sample as *const (), c"(i)i"),
        Native::new(c"ring", bench_ring as *const (), c"(iiiii)"),
        Native::new(c"text", bench_text as *const (), c"(iiiii)"),
        Native::new(c"line_to", bench_line_to as *const (), c"(ii)"),
    ]);
    // SAFETY: registered once, while the table is held.
    unsafe { wamr::register(c"vana", &VANA) && wamr::register(c"bench", &BENCH) }
}
