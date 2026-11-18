use super::Layout;
use super::format::{Goes, Shows};
use super::ready::{Check, Level, Readiness};
use super::ride::summary::{After, Saving, pages};
use super::workout::{Discard, Page, Text, View};
use crate::ride::log::{Sample, Status};
use crate::ride::recovery::Reading as Recovery;
use device_api::input::{Fixture, Fixtures, Role, Side};
use std::boxed::Box;
use std::string::String;
use std::vec::Vec;

/// Every legal arrangement of the three roles' buttons: each role beside a
/// corner of its own, on every panel.
fn every_arrangement() -> Vec<&'static Layout> {
    use super::cue::Corner::{BottomLeft, BottomRight, TopLeft, TopRight};
    let corners = [TopLeft, TopRight, BottomLeft, BottomRight];
    let button = |id, role, corner: super::cue::Corner| Fixture {
        id,
        role: Some(role),
        edge: if corner.left() {
            Side::Left
        } else {
            Side::Right
        },
        at: if corner.top() { 250 } else { 750 },
    };
    let mut layouts = Vec::new();
    for act in corners {
        for back in corners.into_iter().filter(|&back| back != act) {
            for move_ in corners.into_iter().filter(|&it| it != act && it != back) {
                let buttons = Fixtures::new(&[
                    button(0, Role::Act, act),
                    button(1, Role::Back, back),
                    button(2, Role::Move, move_),
                ]);
                for panel in super::panels::ALL {
                    let layout = Layout::of(panel.scaled, &buttons);
                    layouts.push(&*Box::leak(Box::new(layout)));
                }
            }
        }
    }
    layouts
}

fn ridden(layout: &'static Layout, page: Page) -> View<'static> {
    let mut view = View::new(page, 0, None);
    view.layout = layout;
    let sample = Sample {
        speed_mm_s: Some(33_333),
        power_watts: Some(1999),
        heart_bpm: Some(220),
        ..Sample::default()
    };
    for _ in 0..60 {
        view.totals.add(1000, 16_666_666, &sample, None);
        view.zone_time.add(1000, &sample, &view.profile);
    }
    view.active_secs = 359_999;
    view.paused_secs = 359_999;
    view.state = Status::Paused;
    view
}

fn ready(layout: &'static Layout) -> Vec<(String, View<'static>)> {
    let hints = ["CHARGE AFTER THE RIDE", "HR  PWR  SPD/C  RDR"];
    hints
        .map(|hint| {
            let mut readiness = Readiness {
                checks: core::array::from_fn(|_| {
                    Check::new(Level::Wait, "SENSORS WAKING UP", hint)
                }),
                ..Readiness::default()
            };
            let _ = readiness.riding.push_str("RECOVERY SPIN 30");
            let mut view = ridden(layout, Page::Ready);
            view.show_ready(readiness);
            (std::format!("ready, {hint}"), view)
        })
        .into()
}

fn ride_questions(layout: &'static Layout) -> Vec<(String, View<'static>)> {
    let mut screens = Vec::new();
    for auto in [false, true] {
        let mut paused = ridden(layout, Page::Ride);
        paused.pause_screen = true;
        paused.auto_paused = auto;
        screens.push((std::format!("paused, auto {auto}"), paused));
    }
    for (page, discard) in [
        (Page::EndRide, Discard::Unavailable),
        (Page::EndRide, Discard::Offered),
        (Page::DiscardRide, Discard::Offered),
    ] {
        let mut view = ridden(layout, page);
        view.discard = discard;
        screens.push((std::format!("{page:?}, {discard:?}"), view));
    }
    for goes in Goes::ALL {
        let mut view = ridden(layout, Page::Format);
        view.format = Shows::Question(goes);
        screens.push((std::format!("format, {goes:?}"), view));
    }
    for unsaved in [1, 255] {
        let mut view = ridden(layout, Page::LoseRides);
        view.unsaved = unsaved;
        screens.push((std::format!("lose {unsaved} rides"), view));
    }
    let mut bests = ridden(layout, Page::Bests);
    bests.show_bests(Default::default(), super::bests::Mode::Confirm);
    screens.push((String::from("clear the bests"), bests));
    screens
}

fn summaries(
    layout: &'static Layout,
    chart: &'static super::ride::ElevationChart,
) -> Vec<(String, View<'static>)> {
    let offer = crate::workout::ftp::Offer {
        suggested_watts: 1999,
        current_watts: 1999,
        best_watts: 1999,
    };
    let full = After {
        saving: Saving::Saved,
        best: [Some(1999), Some(1500), None, None],
        climbs: Some(999),
        laps: Some(999),
        elevation: true,
        recovery: Some(Recovery::Recovered(120)),
        ..After::EMPTY
    };
    let mut afters = Vec::new();
    for page in 0..=pages(true) {
        afters.push(After {
            page,
            offer: Some(offer),
            ..full
        });
        afters.push(After {
            page,
            past: true,
            send: true,
            delete: true,
            ..full
        });
    }
    afters
        .into_iter()
        .map(|after| {
            let page = if after.past {
                Page::PastRide
            } else {
                Page::Summary
            };
            let mut view = ridden(layout, page);
            view.state = Status::Saved;
            let mut title = Text::new();
            let _ = title.push_str("DEC 31  12:59");
            view.show_summary(after, title);
            view.set_elevation_chart(chart);
            (std::format!("{page:?} page {}", after.page), view)
        })
        .collect()
}

fn checked(broken: &mut Vec<String>, what: String, check: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(check)).is_err() {
        broken.push(what);
    }
}

#[test]
fn every_cue_screen_lays_out_clear_of_its_cues_for_every_button_arrangement() {
    super::check::enable(true);
    let mut trace = crate::ride::elevation::Trace::default();
    for metres in (0..4000).step_by(5) {
        trace.add(metres * 1000, Some(-120 + (metres as i32 % 2000)));
    }
    let mut broken = Vec::new();
    let layouts = every_arrangement();
    assert_eq!(layouts.len(), 24 * super::panels::ALL.len());
    for layout in layouts {
        let chart = Box::leak(Box::default());
        super::ride::ElevationChart::refresh(chart, &trace, layout.summary.elevation.chart);
        let mut screens = ready(layout);
        screens.extend(ride_questions(layout));
        screens.extend(summaries(layout, chart));
        let panel = layout.scaled.panel;
        for (what, mut view) in screens {
            let cues = view.prepare().cues().len();
            assert!(cues > 0, "{what} shows its cues");
            let what = std::format!("{what} on {panel:?}, {:?}", layout.corners);
            checked(&mut broken, what, || view.prepare().check_layout());
        }
        for (what, menu) in super::sensors::tests::crowded(layout) {
            if !menu.cues().is_empty() {
                let what = std::format!("sensors {what} on {panel:?}, {:?}", layout.corners);
                checked(&mut broken, what, || menu.frame().prepare().check_layout());
            }
        }
    }
    assert!(
        broken.is_empty(),
        "under or past a cue:\n{}",
        broken.join("\n")
    );
}
