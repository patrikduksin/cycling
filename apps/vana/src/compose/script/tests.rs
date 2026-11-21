use super::bench;
use super::guests::{
    BENCH, CHATTY, COUNTDOWN, CTOR, FILL, FLAT, NULLSLICE, OUTSIDE, RIGHTEDGE, RINGLEFT, RUNAWAY,
    SPIN, TREND, WIDE, WIDTHS,
};
use super::host::{Call, Job};
use super::{Fault, LIMIT, Refused, STACK, Trap, install};
use crate::compose::data::{Data, Effort, ListKind, Metric};
use crate::compose::input::Controls;
use crate::compose::node::{Flags, Node, Size};
use crate::compose::parse::{self, Screens, VERSION};
use crate::compose::registry::{SCRIPT, VALUE};
use crate::compose::reject::{Place, Reject, Why};
use crate::compose::solve::{Rect16, Solved};
use crate::compose::tile::Form;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::scaled::BASE;
use firmware_shell::rendering::font::SPILL;
use std::boxed::Box;
use std::string::ToString;
use std::time::Instant;
use std::vec::Vec;

fn no_cues(_: u8) -> Controls {
    Controls::FREE
}

/// A record of `trees` as screens 0, 1 and on.
fn record(trees: &[&[Node]]) -> Vec<u8> {
    let mut bytes = std::vec![VERSION, trees.len() as u8];
    for (key, nodes) in trees.iter().enumerate() {
        bytes.extend([key as u8, nodes.len() as u8]);
        bytes.extend(nodes.iter().flat_map(|node| node.bytes()));
    }
    bytes.push(0);
    bytes
}

fn accept_all(trees: &[&[Node]]) -> Result<Box<Screens>, Reject> {
    let mut screens = Box::new(Screens::EMPTY);
    parse::parse(&record(trees), &BASE, &no_cues, &mut screens)?;
    Ok(screens)
}

fn accept(nodes: &[Node]) -> Result<Box<Screens>, Reject> {
    accept_all(&[nodes])
}

fn script(slot: u8) -> Node {
    Node::leaf(SCRIPT, slot, Size::CONTENT)
}

fn frame(screens: &Screens, data: &dyn Data) -> Canvas {
    let mut solved = Solved::EMPTY;
    assert!(screens.solve(0, Flags::NONE, None, &mut solved));
    let mut canvas = Canvas::EMPTY;
    solved.prepare(data, &mut canvas);
    canvas
}

#[derive(Default)]
struct Ride {
    speed: Option<i32>,
    elapsed: Option<i32>,
    /// Watts, one a second, newest last.
    power: Vec<i32>,
}

impl Data for Ride {
    fn metric(&self, metric: Metric) -> Option<i32> {
        match metric {
            Metric::Speed => self.speed,
            Metric::Elapsed => self.elapsed,
            _ => None,
        }
    }
    fn zone(&self, _: Effort) -> Option<u8> {
        None
    }
    fn list(&self, _: ListKind) -> u16 {
        0
    }
    fn sample(&self, metric: Metric, ago: u16) -> Option<i32> {
        let back = self.power.len().checked_sub(1 + usize::from(ago))?;
        (metric == Metric::Power).then(|| self.power[back])
    }
}

fn micros() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

#[test]
fn the_spikes_workloads_give_the_native_checksum_under_wamr() {
    let native = bench::native(&micros);
    let wamr = bench::interpreted(&micros).unwrap();
    assert_eq!(wamr.sum, native.sum);
    assert!(wamr.runaway_stopped, "{wamr:?}");
}

#[test]
fn an_invalid_module_is_refused_with_wamrs_reason() {
    let refused = install(0, b"\0asm\x01\0\0\0\x01\x7f").unwrap_err();
    assert!(matches!(refused, Refused::Invalid(_)), "{refused:?}");
    assert!(
        refused.to_string().starts_with("not a valid module: "),
        "{refused}"
    );
    let refused = install(0, b"tile").unwrap_err().to_string();
    assert_eq!(
        refused,
        "not a valid module: WASM module load failed: magic header not detected"
    );
    assert_eq!(
        install(0, BENCH).unwrap_err().to_string(),
        "imports bench.ring, which VANA does not offer"
    );
    assert_eq!(install(4, COUNTDOWN), Err(Refused::Slot(4)));
    assert_eq!(install(0, &[0; 5000]), Err(Refused::Long(5000)));
}

#[test]
fn a_script_tile_draws_through_the_host_api() {
    install(0, COUNTDOWN).unwrap();
    let screens = accept(&[script(0)]).unwrap();
    let ride = Ride {
        elapsed: Some(25 * 60 + 7),
        ..Ride::default()
    };
    let canvas = frame(&screens, &ride);
    assert!(canvas.shows("25"));
    assert!(matches!(canvas.shapes.as_slice(), [Shape::Ring { .. }]));
    canvas.assert_kept("countdown");
}

#[test]
fn a_runaway_tile_is_stopped_and_the_frame_goes_on() {
    install(1, RUNAWAY).unwrap();
    let speed = Node::leaf(VALUE, Metric::Speed as u8, Size::CONTENT);
    // Accept's trial readings miss the one speed it loops at.
    let screens = accept(&[Node::row(2, Size::grow(1)), speed, script(1)]).unwrap();
    let ride = Ride {
        speed: Some(8_334),
        ..Ride::default()
    };
    for _ in 0..3 {
        let canvas = frame(&screens, &ride);
        assert!(canvas.shows("30.0"), "the speed beside it still shows");
        assert!(canvas.shapes.is_empty(), "the stopped tile left nothing");
    }
}

#[test]
fn a_tile_that_spins_is_refused_at_accept_with_the_reason() {
    install(3, SPIN).unwrap();
    let refused = accept(&[script(3)]).unwrap_err();
    assert_eq!(
        refused,
        Reject {
            place: Place::Record,
            why: Why::Script {
                slot: 3,
                fault: Fault::Stopped {
                    look: Some(0),
                    trap: Trap::Limit
                }
            }
        }
    );
    assert_eq!(
        refused.to_string(),
        "script 3: drawing look 0, it ran past its 2500-instruction limit"
    );
    assert_eq!(LIMIT, 2500);
}

#[test]
fn a_tile_that_draws_past_its_box_is_refused_at_accept() {
    install(3, OUTSIDE).unwrap();
    assert_eq!(
        accept(&[script(3)]).unwrap_err().to_string(),
        "script 3: drawing look 0, it drew outside its box"
    );
}

#[test]
fn accept_caps_the_scripts_a_record_runs_and_a_tree_shows() {
    let why = |nodes: &[Node]| accept(nodes).unwrap_err().why;
    let two = [Node::row(2, Size::grow(1)), script(0), script(1)];
    assert_eq!(
        accept_all(&[&two, &[script(2)]]).unwrap_err().why,
        Why::Scripts(3)
    );
    let thrice = [Node::row(3, Size::grow(1)), script(0), script(0), script(0)];
    assert_eq!(why(&thrice), Why::ScriptTiles(3));
    assert_eq!(
        why(&[script(2)]),
        Why::Script {
            slot: 2,
            fault: Fault::Missing
        }
    );
}

#[test]
fn series_draws_a_window_of_readings_as_bars() {
    install(2, TREND).unwrap();
    let screens = accept(&[script(2)]).unwrap();
    let ride = Ride {
        power: (0..90).map(|second| 100 + 5 * second).collect(),
        ..Ride::default()
    };
    let canvas = frame(&screens, &ride);
    let tops: Vec<usize> = canvas
        .shapes
        .iter()
        .map(|shape| match shape {
            Shape::Bar { rect, .. } => rect.top,
            _ => panic!("a series is bars"),
        })
        .collect();
    assert_eq!(tops.len(), 12);
    assert!(
        tops.windows(2).all(|pair| pair[1] < pair[0]),
        "rising power: {tops:?}"
    );
    assert!(frame(&screens, &Ride::default()).shapes.is_empty());
}

#[test]
fn a_frame_calls_the_instance_accept_made_and_allocates_nothing() {
    install(0, COUNTDOWN).unwrap();
    let screens = accept(&[script(0)]).unwrap();
    let used = wamr::pool_used();
    for elapsed in 0..20 {
        let ride = Ride {
            elapsed: Some(elapsed * 61),
            ..Ride::default()
        };
        frame(&screens, &ride);
    }
    assert_eq!(wamr::pool_used(), used);
}

#[test]
fn the_countdown_tile_benchmarks_on_the_host() {
    let ride = Ride::default();
    let tile = bench::tile(&BASE, &ride, &micros, &|run| {
        run();
        0
    })
    .unwrap();
    assert_eq!((tile.shapes, tile.runs), (1, 1));
}

/// Bytes of a module whose `forms` runs `body` (a function body's code,
/// `end` included) and whose `draw` returns, with one page of memory. It
/// also exports `forms` under each of `aliases` and, with `start`, names it
/// the start function.
fn module(body: &[u8], aliases: &[&str], start: bool) -> Vec<u8> {
    fn section(id: u8, content: &[u8]) -> Vec<u8> {
        let mut bytes = std::vec![id, content.len() as u8];
        bytes.extend(content);
        bytes
    }
    let mut exports = std::vec![2 + aliases.len() as u8];
    for (name, func) in [("forms", 0), ("draw", 1)]
        .into_iter()
        .chain(aliases.iter().map(|&name| (name, 0)))
    {
        exports.push(name.len() as u8);
        exports.extend(name.bytes());
        exports.extend([0, func]);
    }
    let mut code = std::vec![2, body.len() as u8 + 1, 0];
    code.extend(body);
    code.extend([2, 0, 0x0b]);
    let mut bytes = b"\0asm\x01\0\0\0".to_vec();
    bytes.extend(section(1, &[2, 0x60, 0, 0, 0x60, 3, 0x7f, 0x7f, 0x7f, 0]));
    bytes.extend(section(3, &[2, 0, 1]));
    bytes.extend(section(5, &[1, 0, 1]));
    bytes.extend(section(7, &exports));
    if start {
        bytes.extend(section(8, &[0]));
    }
    bytes.extend(section(10, &code));
    bytes
}

/// `loop br 0 end end`: spins for ever.
const SPINS: &[u8] = &[0x03, 0x40, 0x0c, 0x00, 0x0b, 0x0b];

#[test]
fn a_module_that_runs_code_as_it_is_instantiated_is_refused_at_install() {
    assert_eq!(install(0, &module(SPINS, &[], false)), Ok(()));
    assert_eq!(
        install(0, &module(SPINS, &[], true))
            .unwrap_err()
            .to_string(),
        "runs its start function when instantiated"
    );
    for entry in ["__post_instantiate", "__wasm_call_ctors", "_initialize"] {
        assert_eq!(
            install(0, &module(SPINS, &[entry], false)),
            Err(Refused::Starts(entry))
        );
    }
    assert_eq!(
        install(2, CTOR).unwrap_err().to_string(),
        "runs __post_instantiate when instantiated"
    );
    assert_eq!(
        accept(&[script(2)]).unwrap_err().why,
        Why::Script {
            slot: 2,
            fault: Fault::Missing
        }
    );
}

#[test]
fn a_module_that_fills_or_copies_memory_or_a_table_in_one_instruction_is_refused_at_install() {
    // Three i32.consts, then memory.fill, memory.copy or table.copy.
    let operands = [0x41, 0, 0x41, 0, 0x41, 10];
    for (op, name) in [
        (&[0xfc, 0x0b, 0][..], "fc 0b"),
        (&[0xfc, 0x0a, 0, 0], "fc 0a"),
        (&[0xfc, 0x0e, 0, 0], "fc 0e"),
    ] {
        let body = [&operands[..], op, &[0x0b]].concat();
        let refused = install(0, &module(&body, &[], false))
            .unwrap_err()
            .to_string();
        assert!(
            refused.ends_with(&std::format!("unsupported opcode {name}")),
            "{refused}"
        );
    }
    // ref.null, which table.fill and table.grow take, is gone with them.
    let body = [0x41, 0, 0xd0, 0x70, 0x41, 1, 0xfc, 0x11, 0, 0x0b];
    let refused = install(0, &module(&body, &[], false))
        .unwrap_err()
        .to_string();
    assert!(refused.ends_with("unsupported opcode d0"), "{refused}");
}

/// `wasm`'s `draw` of look `look` into `rect`, outside accept: its result,
/// what it drew and how long it took.
fn draw_once(
    wasm: &[u8],
    rect: Rect16,
    form: Form,
    look: u32,
    data: &dyn Data,
) -> (Result<(), Trap>, Canvas, std::time::Duration) {
    super::runtime(|floor| {
        // SAFETY: the script table is held for the whole closure.
        let module = unsafe { wamr::Module::load(wasm) }.unwrap();
        let mut instance = unsafe { wamr::Instance::new(&module, STACK, 1, floor) }.unwrap();
        let draw = instance.func(c"draw").unwrap();
        let mut canvas = Canvas::EMPTY;
        let mut args = [look, u32::from(rect.width()), u32::from(rect.height())];
        let start = Instant::now();
        let result = Call::new(&BASE, Job::draw(rect, &form, data, &mut canvas)).run(
            &mut instance,
            draw,
            &mut args,
        );
        let took = start.elapsed();
        drop(instance);
        (result, canvas, took)
    })
    .unwrap()
}

const BOX: Rect16 = Rect16 {
    left: 20,
    top: 20,
    right: 120,
    bottom: 120,
};

#[test]
fn right_aligned_text_in_a_box_narrower_than_its_spill_fits_as_no_text() {
    install(0, RIGHTEDGE).unwrap();
    accept(&[script(0)]).unwrap();
    let form = super::forms(0)[0];
    let rect = Rect16 {
        right: BOX.left + form.width,
        bottom: BOX.top + form.height,
        ..BOX
    };
    let (result, canvas, _) = draw_once(RIGHTEDGE, rect, form, 0, &Ride::default());
    assert_eq!(result, Ok(()));
    assert!(canvas.runs.iter().all(|run| run.line.text().is_empty()));
}

#[test]
fn empty_text_at_the_end_of_memory_measures_as_no_text() {
    install(1, NULLSLICE).unwrap();
    accept(&[script(1)]).unwrap();
    let width = super::forms(1)[0].width;
    assert_eq!(usize::from(width), 100 + SPILL);
}

#[test]
fn a_ring_past_its_boxs_top_left_is_refused() {
    let form = Form {
        width: 100,
        height: 100,
        shapes: 1,
        runs: 0,
    };
    let (result, canvas, _) = draw_once(RINGLEFT, BOX, form, 0, &Ride::default());
    assert_eq!(result, Err(Trap::Outside));
    assert!(canvas.shapes.is_empty());
}

#[test]
fn a_series_needs_a_row_and_a_column_for_each_bar() {
    let form = Form {
        width: 100,
        height: 100,
        shapes: 2,
        runs: 0,
    };
    let ride = Ride {
        power: (0..90).map(|second| 100 + second).collect(),
        ..Ride::default()
    };
    for look in [0, 1] {
        let (result, canvas, _) = draw_once(FLAT, BOX, form, look, &ride);
        assert_eq!(result, Err(Trap::Outside), "look {look}");
        assert!(canvas.shapes.is_empty(), "look {look}");
    }
}

#[test]
fn a_native_reads_at_most_the_characters_a_line_draws() {
    let long = [b'8'; 60];
    // SAFETY: `long` holds 60 bytes, and an empty text is never read.
    let (text, empty) = unsafe {
        (
            super::host::guest_text(long.as_ptr(), 60),
            super::host::guest_text(core::ptr::null(), 0),
        )
    };
    assert_eq!(text.len(), super::host::LONGEST);
    assert!(empty.is_empty());
}

#[test]
fn a_draw_that_measures_or_fills_over_and_over_stops_about_as_fast_as_a_spin() {
    let form = Form {
        width: 100,
        height: 100,
        shapes: 0,
        runs: 0,
    };
    let best = |wasm| {
        (0..5)
            .map(|_| draw_once(wasm, BOX, form, 0, &Ride::default()))
            .map(|(result, _, took)| {
                assert_eq!(result, Err(Trap::Limit));
                took
            })
            .min()
            .unwrap()
    };
    let spin = best(SPIN);
    for (name, wasm) in [("widths", WIDTHS), ("fill", FILL)] {
        let took = best(wasm);
        assert!(
            took < 3 * spin,
            "{name}: {took:?} against a spin's {spin:?}"
        );
    }
}

#[test]
fn accept_draws_each_look_with_every_reading_at_its_ends() {
    install(0, WIDE).unwrap();
    assert_eq!(
        accept(&[script(0)]).unwrap_err().to_string(),
        "script 0: drawing look 0, it drew outside its box"
    );
}

#[test]
fn a_draw_that_calls_one_native_over_and_over_runs_out_of_host_work() {
    let form = Form {
        width: 200,
        height: 200,
        shapes: 255,
        runs: 255,
    };
    let rect = Rect16 {
        left: 20,
        top: 20,
        right: 220,
        bottom: 220,
    };
    let ride = Ride {
        power: (0..700).collect(),
        ..Ride::default()
    };
    let best = |wasm, native| {
        (0..5)
            .map(|_| draw_once(wasm, rect, form, native, &ride))
            .map(|(result, _, took)| {
                assert_eq!(result, Err(Trap::Limit), "native {native}");
                took
            })
            .min()
            .unwrap()
    };
    let spin = best(SPIN, 0);
    for native in 0..9 {
        let took = best(CHATTY, native);
        assert!(
            took < 3 * spin,
            "native {native}: {took:?} against a spin's {spin:?}"
        );
    }
}
