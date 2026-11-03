//! Discarding a ride at its end and deleting a saved ride: the journals keep
//! every ride, and `rides/deleted.tbl` on FILES hides it from the list, the
//! ride count, the week's totals and the phone.
#![cfg(feature = "cycling")]
use device_api::input::Button;
use device_api::server::Method;
use shell_simulator::ant::device_type;
use shell_simulator::bulk::Presence;
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{Sim, minutes, seconds};
use vana::ride::deleted::{self, Reason};

const HEART: u16 = 4242;
const POWER: u16 = 1234;
const SPEED: u16 = 777;

/// A card with a formatted FILES store, the sensors paired and the clock
/// set; rides go to the card's log unless `internal`.
fn rider(name: &str, internal: bool) -> Sim {
    let mut sim = Sim::named(name);
    sim.fit_bulk(Presence::Ready);
    let mut sim = sim.boot();
    sim.ant_sensor(device_type::HEART_RATE, HEART);
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED);
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    if !internal {
        sim.init_bulk();
    }
    sim.init_volumes();
    sim.put_file("notes.txt", b"x");
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.set_gps_clock(15 * 3600, (21, 10, 26));
    sim
}

/// A ride at `kmh` and `watts` for `length`, saved, back Home and standing.
fn ride(sim: &mut Sim, kmh: f64, watts: f64, length: u64) {
    sim.ride_at(kmh, watts, 140.0, 88.0);
    sim.start_ride();
    sim.advance(length);
    sim.stop_ride();
    sim.done();
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
}

/// Until the minute turns, so Home draws again and any toast has gone.
fn home_frame(sim: &mut Sim) {
    sim.advance(seconds(61));
}

fn hidden(sim: &mut Sim) -> Vec<deleted::Record> {
    sim.file(deleted::TABLE)
        .map(|bytes| deleted::decode_table(&bytes).collect())
        .unwrap_or_default()
}

/// The Home RIDES card's line, such as "WEEK 9.3 KM".
fn rides_line(sim: &Sim) -> String {
    let text = sim.screen().text();
    let mut lines = text.lines().skip_while(|line| *line != "RIDES");
    lines
        .nth(1)
        .unwrap_or_else(|| panic!("no RIDES card; screen:\n{text}"))
        .to_string()
}

/// From a saved ride's first page, MOVE until DELETE RIDE? shows.
fn to_delete_page(sim: &mut Sim) {
    sim.wait_gone("READING", seconds(5)).unwrap();
    for _ in 0..6 {
        if sim.screen().contains("DELETE RIDE?") {
            return;
        }
        sim.press(Button::BottomLeft);
    }
    panic!("no DELETE RIDE?; screen:\n{}", sim.screen().text());
}

fn activity_ids(sim: &mut Sim) -> Vec<String> {
    let (code, body) = sim.phone_request(Method::Get, "/api/activities", b"");
    assert_eq!(code, 200, "{body}");
    body.split("\"id\":\"")
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap().to_string())
        .collect()
}

#[test]
fn a_ride_discarded_after_a_lap_is_saved_then_hidden_and_sets_no_best() {
    let mut sim = rider("discard_after_a_lap", false);
    ride(&mut sim, 28.0, 210.0, minutes(20));
    home_frame(&mut sim);
    let week = rides_line(&sim);
    let bests = sim.file("bests.dat");
    sim.checkpoint("home before the discarded ride");
    // The screen dimmed: the first press only wakes it.
    sim.press(Button::BottomLeft);

    sim.ride_at(34.0, 420.0, 170.0, 95.0);
    sim.start_ride();
    sim.advance(minutes(6));
    sim.lap(1);
    sim.advance(minutes(2));
    sim.pause_ride();
    sim.press(Button::TopLeft);
    sim.expect_text("END RIDE?");
    sim.checkpoint("end ride with the discard cue on move");
    sim.press(Button::BottomLeft);
    sim.expect_text("DISCARD RIDE?");
    sim.expect_text("IT WILL NOT BE SAVED");
    sim.checkpoint("discard ride asks");
    sim.press(Button::TopLeft);
    sim.expect_text("END RIDE?");
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomRight);
    sim.wait_for("RIDE DISCARDED", seconds(5)).unwrap();
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("home says ride discarded");

    assert_eq!(sim.bulk_rides().len(), 2, "the journal keeps the ride");
    let records = hidden(&mut sim);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].reason, Reason::Discarded);
    assert_eq!(records[0].ride.ride_id, sim.bulk_rides()[1].id);
    assert_eq!(sim.file("bests.dat"), bests, "the 420 W ride set no best");
    let facts = sim.file(vana::ride::facts::TABLE).unwrap();
    assert!(
        vana::ride::facts::decode_table(&facts).all(|r| r.ride_id != records[0].ride.ride_id),
        "no facts for the discarded ride"
    );
    home_frame(&mut sim);
    assert_eq!(rides_line(&sim), week, "the week leaves it out");

    sim.power_loss();
    home_frame(&mut sim);
    assert_eq!(rides_line(&sim), week, "and still after a restart");
    sim.home("RIDES");
    sim.advance(seconds(5));
    sim.expect_text("1/1");
}

#[test]
fn an_auto_paused_ride_and_a_zero_km_test_ride_discard_the_same_way() {
    let mut sim = rider("discard_auto_paused_and_zero_km", false);
    sim.ride_at(30.0, 220.0, 140.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(3));
    sim.ride_at(0.0, 0.0, 120.0, 0.0);
    sim.wait_for("AUTO PAUSED", seconds(30)).unwrap();
    sim.press(Button::TopLeft);
    sim.expect_text("END RIDE?");
    sim.ride_at(30.0, 220.0, 140.0, 90.0);
    sim.advance(seconds(10));
    sim.expect_text("END RIDE?");
    sim.press(Button::BottomLeft);
    sim.checkpoint("auto-paused ride discard ride asks");
    sim.press(Button::BottomRight);
    sim.wait_for("RIDE DISCARDED", seconds(5)).unwrap();

    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(15));
    if sim.ride_state() == "recording" {
        sim.pause_ride();
    }
    sim.press(Button::TopLeft);
    sim.press(Button::BottomLeft);
    sim.expect_text("0.00 KM");
    sim.checkpoint("zero km test ride discard ride asks");
    sim.press(Button::BottomRight);
    sim.wait_for("RIDE DISCARDED", seconds(5)).unwrap();

    assert_eq!(sim.bulk_rides().len(), 2);
    assert_eq!(hidden(&mut sim).len(), 2);
    home_frame(&mut sim);
    assert_eq!(rides_line(&sim), "NONE YET");
}

/// The ride is saved before it is hidden: a card that fails between the two,
/// as when the power goes, leaves an ordinary saved ride.
#[test]
fn a_discard_cut_off_before_the_mark_leaves_a_saved_ride() {
    let mut sim = rider("discard_cut_before_the_mark", true);
    sim.ride_at(30.0, 220.0, 140.0, 90.0);
    sim.start_ride();
    sim.advance(minutes(4));
    sim.pause_ride();
    sim.press(Button::TopLeft);
    sim.press(Button::BottomLeft);
    sim.expect_text("DISCARD RIDE?");
    sim.fail_bulk(0, 100);
    sim.press(Button::BottomRight);
    sim.wait_for("NOT DISCARDED", seconds(5)).unwrap();
    assert!(sim.on_home(), "{}", sim.screen().text());
    assert_eq!(sim.rides().len(), 1, "saved on internal flash");

    sim.power_loss();
    assert!(hidden(&mut sim).is_empty(), "nothing marked");
    home_frame(&mut sim);
    assert_eq!(rides_line(&sim), "WEEK 2 KM");
    sim.home("RIDES");
    sim.advance(seconds(5));
    sim.expect_text("1/1");
}

#[test]
fn a_deleted_ride_leaves_the_list_the_count_the_week_and_the_phone() {
    let mut sim = rider("delete_a_saved_ride", false);
    ride(&mut sim, 28.0, 210.0, minutes(20));
    ride(&mut sim, 0.0, 0.0, seconds(40));
    ride(&mut sim, 30.0, 230.0, minutes(10));
    let rides = sim.bulk_rides();
    assert_eq!(rides.len(), 3);
    sim.connect_phone();
    let before = activity_ids(&mut sim);
    assert_eq!(before.len(), 3, "{before:?}");
    sim.press(Button::TopLeft);
    while !sim.on_home() {
        sim.press(Button::TopLeft);
    }
    home_frame(&mut sim);
    assert_eq!(rides_line(&sim), "WEEK 14 KM");

    sim.home("RIDES");
    sim.advance(seconds(5));
    sim.expect_text("1/3");
    sim.press(Button::BottomLeft);
    sim.press(Button::BottomRight);
    sim.wait_gone("READING", seconds(5)).unwrap();
    sim.checkpoint("saved ride with one more page dot");
    to_delete_page(&mut sim);
    sim.expect_text("IT GOES FROM YOUR RIDES");
    sim.checkpoint("delete ride asks");
    sim.press(Button::BottomLeft);
    sim.expect_no_text("DELETE RIDE?");
    to_delete_page(&mut sim);
    sim.press(Button::BottomRight);
    sim.wait_for("RIDE DELETED", seconds(5)).unwrap();
    sim.checkpoint("rides list says ride deleted");
    sim.expect_text("2/2");

    let records = hidden(&mut sim);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].reason, Reason::Deleted);
    assert_eq!(records[0].ride.ride_id, rides[1].id, "the 40 s test ride");
    assert_eq!(sim.bulk_rides().len(), 3, "nothing erased");

    // The newest ride, which Home's card counted last: MOVE wraps to it.
    sim.press(Button::BottomLeft);
    sim.expect_text("1/2");
    sim.press(Button::BottomRight);
    to_delete_page(&mut sim);
    sim.press(Button::BottomRight);
    sim.wait_for("RIDE DELETED", seconds(5)).unwrap();
    sim.expect_text("1/1");
    sim.expect_text("0:20:00");
    sim.press(Button::TopLeft);
    home_frame(&mut sim);
    assert_eq!(rides_line(&sim), "WEEK 9 KM");

    sim.connect_phone();
    let ids = activity_ids(&mut sim);
    assert_eq!(ids.len(), 1, "{ids:?}");
    for gone in before.iter().filter(|id| !ids.contains(id)) {
        let (code, body) =
            sim.phone_request(Method::Get, &format!("/api/activities/{gone}.fit"), b"");
        assert_eq!(
            (code, body.contains("deleted")),
            (404, true),
            "{gone}: {body}"
        );
    }

    sim.power_loss();
    home_frame(&mut sim);
    assert_eq!(rides_line(&sim), "WEEK 9 KM", "hidden after a restart");
}
