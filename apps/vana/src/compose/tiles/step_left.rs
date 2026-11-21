//! What is left of the workout step, over the step's progress bar, amber
//! for the last seconds. A tap adds to the step, as holding MOVE does.
use crate::compose::data::Data;
use crate::compose::input::Action;
use crate::compose::solve::Placed;
use crate::compose::tile::{Form, Forms, Tile};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{AMBER, Align, Rect, TERTIARY, WHITE};
use crate::screens::ride::workout::clock;
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use crate::workout::model::Intensity;
use crate::workout::player::{COUNTDOWN_S, Left};
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

pub struct StepLeft;

const FACES: [FaceRole; 2] = [FaceRole::Value, FaceRole::Body];
/// Rows from the value's baseline to the bar, and the bar's own.
const UNDER_VALUE_UNITS: usize = 7;
const BAR_UNITS: usize = 5;
const WORDS_GAP_UNITS: usize = 6;

impl Tile for StepLeft {
    type Arg = ();
    const NAME: &'static str = "step left";

    /// Measured on a step under ten minutes; a longer one steps the value
    /// down a face and cuts the words beside it, as it always did.
    fn forms((): (), at: &Scaled) -> Forms {
        let caption = at.face(FaceRole::Caption);
        FACES
            .iter()
            .map(|&role| {
                let face = at.face(role);
                Form {
                    width: (face.fit_width(b"9:59")
                        + at.unit(WORDS_GAP_UNITS)
                        + caption.fit_width(b"LAP TO START")) as u16,
                    height: (face.fit_height() + at.unit(UNDER_VALUE_UNITS + BAR_UNITS)) as u16,
                    shapes: 2,
                    runs: 2,
                }
            })
            .collect()
    }

    fn action((): ()) -> Option<Action> {
        Some(Action::ExtendStep)
    }

    fn prepare((): (), placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let Some(training) = data.training().filter(|training| !training.done) else {
            return;
        };
        let now = &training.now;
        let rect = placed.rect.rect();
        let bar = Rect {
            top: rect.bottom - at.unit(BAR_UNITS),
            ..rect
        };
        let base = bar.top - at.unit(UNDER_VALUE_UNITS);
        let mut left = Text::<12>::new();
        let (words, last): (&[u8], bool) = match now.left {
            Left::Secs(secs) => {
                clock(&mut left, secs);
                (b"LEFT", secs <= COUNTDOWN_S + 2 && data.recording())
            }
            Left::Metres(m) => {
                let _ = write!(left, "{}.{:02}", m / 1000, m % 1000 / 10);
                (b"KM LEFT", false)
            }
            Left::Open(secs) => {
                clock(&mut left, secs);
                let starts_test = now
                    .next
                    .is_some_and(|next| next.step.intensity == Intensity::Test);
                (
                    if starts_test {
                        b"LAP TO START"
                    } else {
                        b"LAP TO END"
                    },
                    false,
                )
            }
        };
        let face = at.face(FACES[usize::from(placed.form)]);
        let value = out.fit(
            &[face, at.face(FaceRole::Body)],
            left.as_bytes(),
            rect.width() / 2,
            rect.left,
            base,
            Align::Left,
            if last { AMBER } else { WHITE },
        );
        out.caption(
            at,
            words,
            rect.right
                .saturating_sub(value.right() + at.unit(2 * WORDS_GAP_UNITS)),
            value.right() + at.unit(WORDS_GAP_UNITS),
            base,
            Align::Left,
        );
        let radius = at.unit(2);
        out.shape(Shape::Bar {
            rect: bar,
            radius,
            color: TERTIARY,
        });
        let lit = rect.left + rect.width() * usize::from(now.progress) / 1000;
        if lit > rect.left + at.unit(4) {
            out.shape(Shape::Bar {
                rect: Rect { right: lit, ..bar },
                radius,
                color: WHITE,
            });
        }
    }
}
