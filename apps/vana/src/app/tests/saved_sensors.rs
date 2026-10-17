//! Saved ANT sensors: restore, automatic reconnection, removal and the
//! bounded scheduling that serves them.
use super::*;
use crate::screens::sensors::{Connection, MenuMessage};
use crate::screens::workout::Page;
use device_api::ant::{LinkState, Request};

fn peer(device_type: u8, device_number: u16) -> Identity {
    Identity {
        device_type,
        device_number,
        transmission_type: 1,
    }
}

fn connection(runtime: &Runtime, radio: &Radio, now: u64, kind: u8) -> Connection {
    runtime
        .sensor_tiles(radio, now)
        .into_iter()
        .flatten()
        .find(|tile| tile.identity.device_type == kind)
        .expect("saved tile")
        .connection
}

fn link(radio: &Radio, kind: u8, now: u64) -> Option<LinkState> {
    radio.channel(kind, now).map(|s| s.link)
}

/// Let the radio and runtime run in 10 ms passes, like the firmware loop.
fn run(runtime: &mut Runtime, radio: &mut Radio, from: u64, to: u64) {
    for now in (from..to).step_by(10) {
        radio.channels.tick(now);
        advance(runtime, radio, now);
    }
}

const HEART: u8 = 120;
const POWER: u8 = 11;

#[test]
fn restored_sensors_connect_at_startup_without_opening_sensors() {
    let (heart, power) = (peer(HEART, 4242), peer(POWER, 1234));
    let mut runtime = restored(&[heart, power]);
    let mut radio = Radio::new();
    assert!(runtime.navigation.page == Page::Boot);
    // One request per pass.
    advance(&mut runtime, &mut radio, 0);
    assert_eq!(radio.requests.len(), 1);
    advance(&mut runtime, &mut radio, 10);
    assert_eq!((radio.connects(heart), radio.connects(power)), (1, 1));
    assert_eq!(
        connection(&runtime, &radio, 20, HEART),
        Connection::WaitingForData
    );
    radio.data(HEART, [0; 8], 30);
    assert_eq!(connection(&runtime, &radio, 40, HEART), Connection::Live);
    // Nothing else is requested once connected.
    run(&mut runtime, &mut radio, 20, 5000);
    assert_eq!(radio.requests.len(), 2);
    assert!(
        !radio
            .requests
            .iter()
            .any(|op| matches!(op, AntOperation::Scan(_)))
    );
}

#[test]
fn startup_waits_for_the_radio_then_connects() {
    let heart = peer(HEART, 4242);
    let mut runtime = restored(&[heart]);
    let mut radio = Radio::new();
    radio.availability = Availability::Initializing;
    run(&mut runtime, &mut radio, 0, 3000);
    assert!(radio.requests.is_empty());
    assert_eq!(
        connection(&runtime, &radio, 3000, HEART),
        Connection::WaitingForRadio
    );
    radio.availability = Availability::Ready;
    advance(&mut runtime, &mut radio, 3000);
    assert_eq!(radio.connects(heart), 1);
}

#[test]
fn late_waking_sensor_connects_after_timeouts_with_bounded_backoff() {
    let heart = peer(HEART, 4242);
    let mut runtime = restored(&[heart]);
    let mut radio = Radio::quiet();
    let mut attempts = std::vec::Vec::new();
    let mut seen = 0;
    let mut statuses = std::vec::Vec::new();
    for now in (0..300_000).step_by(10) {
        radio.channels.tick(now);
        advance(&mut runtime, &mut radio, now);
        if radio.requests.len() > seen {
            seen = radio.requests.len();
            assert!(matches!(radio.requests.last(), Some(AntOperation::Connect(p)) if *p == heart));
            attempts.push(now);
        }
        statuses.push(connection(&runtime, &radio, now, HEART));
    }
    // The transport times out each search after ten seconds; retries wait
    // 1, 2, 4, 8, 16 and then 30 seconds more, never less.
    let gaps: std::vec::Vec<u64> = attempts.windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(
        &gaps[..6],
        &[11_000, 12_000, 14_000, 18_000, 26_000, 40_000]
    );
    assert!(gaps[6..].iter().all(|gap| *gap == 40_000), "{gaps:?}");
    assert!(attempts.len() <= 12, "{attempts:?}");
    assert!(statuses.contains(&Connection::Searching));
    assert!(statuses.contains(&Connection::NotFound));
    // The sensor wakes while the next search is open.
    let mut now = 300_000;
    while link(&radio, HEART, now) != Some(LinkState::Connecting) {
        assert!(now < 345_000, "no search within the longest delay");
        radio.channels.tick(now);
        advance(&mut runtime, &mut radio, now);
        now += 10;
    }
    let now = now - 41_000;
    radio
        .channels
        .receive(Event::Connected(heart), now + 41_000);
    radio.data(HEART, [0; 8], now + 41_100);
    advance(&mut runtime, &mut radio, now + 41_200);
    assert_eq!(
        connection(&runtime, &radio, now + 41_200, HEART),
        Connection::Live
    );
    // Fresh data reset the backoff: after the next loss the search reopens
    // at once, and only once.
    let count = radio.connects(heart);
    radio.channels.receive(Event::Timeout(heart), now + 42_000);
    run(&mut runtime, &mut radio, now + 42_000, now + 42_010);
    assert_eq!(radio.connects(heart), count + 1);
    run(&mut runtime, &mut radio, now + 42_010, now + 51_000);
    assert_eq!(radio.connects(heart), count + 1);
}

#[test]
fn loss_and_return_reconnect_only_the_saved_identity() {
    let power = peer(POWER, 1234);
    let other = peer(POWER, 999);
    let mut runtime = restored(&[power]);
    let mut radio = Radio::quiet();
    advance(&mut runtime, &mut radio, 0);
    radio.channels.receive(Event::Connected(power), 100);
    radio.data(POWER, [0x10, 1, 0, 90, 0, 0, 200, 0], 200);
    advance(&mut runtime, &mut radio, 300);
    // Another power meter is nearby and discovered.
    radio.channels.begin_scan(400, 1000).unwrap();
    radio.channels.receive(
        Event::Discovery {
            identity: other,
            rssi: -30,
        },
        500,
    );
    // The companion reports a close, then later a timeout.
    radio.channels.receive(Event::Disconnected(power), 2000);
    run(&mut runtime, &mut radio, 2000, 4000);
    assert_eq!(link(&radio, POWER, 4000), Some(LinkState::Connecting));
    run(&mut runtime, &mut radio, 4000, 40_000);
    radio.channels.receive(Event::Connected(power), 40_000);
    advance(&mut runtime, &mut radio, 40_000);
    assert_eq!(link(&radio, POWER, 40_000), Some(LinkState::Connected));
    assert_eq!(radio.connects(other), 0);
    assert!(radio.requests.iter().all(|op| match op {
        AntOperation::Connect(p) => *p == power,
        _ => true,
    }));
}

#[test]
fn transport_loss_waits_for_a_ready_radio_and_uncertain_discovery_is_not_retried() {
    let heart = peer(HEART, 4242);
    let mut runtime = restored(&[heart]);
    let mut radio = Radio::new();
    advance(&mut runtime, &mut radio, 0);
    radio.data(HEART, [0; 8], 10);
    // The companion restarts: every channel is lost and the radio starts over.
    radio.channels.transport_loss(1000);
    radio.availability = Availability::Initializing;
    run(&mut runtime, &mut radio, 1000, 6000);
    assert_eq!(radio.connects(heart), 1);
    assert_eq!(
        connection(&runtime, &radio, 6000, HEART),
        Connection::WaitingForRadio
    );
    radio.availability = Availability::Ready;
    advance(&mut runtime, &mut radio, 6000);
    assert_eq!(radio.connects(heart), 2);
    // A failed scan leaves discovery uncertain: the radio cannot connect
    // anything until a confirmed restart, so nothing is requested.
    radio.channels.transport_loss(7000);
    radio
        .channels
        .request_failed(Request::Scan { duration_ms: 1000 }, 7000);
    radio.channels.begin_scan(7000, 1000).unwrap_err();
    run(&mut runtime, &mut radio, 7000, 20_000);
    assert_eq!(radio.connects(heart), 2);
    assert_eq!(
        connection(&runtime, &radio, 20_000, HEART),
        Connection::RadioUnavailable
    );
    radio.channels.companion_restarted(20_000);
    run(&mut runtime, &mut radio, 20_000, 20_100);
    assert_eq!(radio.connects(heart), 3);
}

#[test]
fn connection_work_continues_with_the_display_off_and_after_wake() {
    let heart = peer(HEART, 4242);
    let mut runtime = restored(&[heart]);
    let mut radio = Radio::new();
    runtime.suspend_display();
    advance(&mut runtime, &mut radio, 0);
    assert_eq!(radio.connects(heart), 1);
    radio.data(HEART, [0; 8], 400);
    advance(&mut runtime, &mut radio, 500);
    // The device sleeps: nothing runs, and the radio loses its channels.
    radio.channels.transport_loss(1000);
    // On wake the loop runs again and reconnects at once.
    advance(&mut runtime, &mut radio, 600_000);
    assert_eq!(radio.connects(heart), 2);
    assert_eq!(link(&radio, HEART, 600_000), Some(LinkState::Connected));
}

#[test]
fn disconnect_removes_closes_and_late_events_cannot_resurrect_it() {
    let heart = peer(HEART, 4242);
    let mut runtime = restored(&[heart]);
    let mut radio = Radio::quiet();
    advance(&mut runtime, &mut radio, 0);
    assert_eq!(link(&radio, HEART, 0), Some(LinkState::Connecting));
    // Disconnect while the connection is still pending.
    runtime.navigation.page = Page::Sensors;
    runtime.input(press(Button::BottomLeft), 1000, &mut radio);
    runtime.input(press(Button::BottomRight), 1400, &mut radio);
    runtime.input(press(Button::BottomRight), 1800, &mut radio);
    assert_eq!(runtime.links.saved.of_type(HEART), None);
    assert_eq!(runtime.preferences.sensors.of_type(HEART), None);
    assert!(
        runtime
            .sensor_tiles(&radio, 1800)
            .iter()
            .all(Option::is_none)
    );
    // The scan the page started was cancelled for the close; once it stops,
    // the pending session closes.
    run(&mut runtime, &mut radio, 1800, 6000);
    assert!(
        radio
            .requests
            .iter()
            .any(|op| matches!(op, AntOperation::Disconnect(HEART)))
    );
    assert_eq!(link(&radio, HEART, 6000), Some(LinkState::Disconnecting));
    // A late open is ignored; pages never reach the ride.
    radio.channels.receive(Event::Connected(heart), 6100);
    radio.data(HEART, [0, 0, 0, 0, 0, 0, 1, 80], 6100);
    assert!(runtime.links.rejects(heart));
    radio.channels.receive(Event::Disconnected(heart), 6200);
    let connects = radio.connects(heart);
    run(&mut runtime, &mut radio, 6200, 120_000);
    assert_eq!(radio.connects(heart), connects);
    // A restart with the saved record does not reconnect it either.
    let record = preferences::encode(runtime.preferences.profile, &runtime.preferences.sensors);
    let mut restarted = Runtime::new(Profile::HeartRate);
    restarted.load_preferences(&firmware_shell::preferences::AppPreferences::new(&record).unwrap());
    let mut fresh = Radio::new();
    run(&mut restarted, &mut fresh, 0, 30_000);
    assert_eq!(fresh.connects(heart), 0);
}

#[test]
fn disconnect_of_a_connected_sensor_retries_a_busy_close() {
    let heart = peer(HEART, 4242);
    let mut runtime = restored(&[heart]);
    let mut radio = Radio::new();
    advance(&mut runtime, &mut radio, 0);
    radio.data(HEART, [0; 8], 10);
    radio.reject = Some(device_api::ant::Error::Busy);
    runtime.request_sensor(
        crate::screens::sensors::Action::Disconnect(HEART),
        &mut radio,
        100,
    );
    assert_eq!(link(&radio, HEART, 100), Some(LinkState::Connected));
    radio.reject = None;
    run(&mut runtime, &mut radio, 100, 1000);
    assert_eq!(link(&radio, HEART, 1000), Some(LinkState::Disconnected));
    assert_eq!(radio.connects(heart), 1);
}

#[test]
fn reselecting_a_removed_sensor_connects_it_again() {
    let heart = peer(HEART, 4242);
    let mut runtime = restored(&[heart]);
    let mut radio = Radio::new();
    advance(&mut runtime, &mut radio, 0);
    runtime.request_sensor(
        crate::screens::sensors::Action::Disconnect(HEART),
        &mut radio,
        100,
    );
    run(&mut runtime, &mut radio, 100, 500);
    assert_eq!(link(&radio, HEART, 500), Some(LinkState::Disconnected));
    runtime.request_sensor(
        crate::screens::sensors::Action::Connect(heart),
        &mut radio,
        600,
    );
    assert_eq!(link(&radio, HEART, 600), Some(LinkState::Connected));
    assert!(!runtime.links.rejects(heart));
    assert_eq!(runtime.preferences.sensors.of_type(HEART), Some((0, heart)));
}

#[test]
fn ten_saved_sensors_are_served_in_turn_without_duplicate_requests() {
    let peers: std::vec::Vec<Identity> = (1..=10).map(|kind| peer(kind, 100)).collect();
    let mut runtime = restored(&peers);
    let mut radio = Radio::quiet();
    // A busy radio refuses everything for a while.
    radio.reject = Some(device_api::ant::Error::Busy);
    run(&mut runtime, &mut radio, 0, 3000);
    radio.requests.clear();
    radio.reject = None;
    run(&mut runtime, &mut radio, 3000, 5100);
    for peer in &peers {
        assert_eq!(radio.connects(*peer), 1, "{peer:?}");
    }
    assert_eq!(radio.requests.len(), 10);
    // All time out together; each is searched once more after its delay.
    run(&mut runtime, &mut radio, 5100, 16_100);
    for peer in &peers {
        assert_eq!(radio.connects(*peer), 2, "{peer:?}");
    }
    // Established reception continues while the others retry.
    radio.channels.receive(Event::Connected(peers[0]), 16_200);
    for now in (16_300..40_000).step_by(250) {
        radio.data(1, [0; 8], now);
        radio.channels.tick(now);
        advance(&mut runtime, &mut radio, now);
    }
    assert_eq!(connection(&runtime, &radio, 40_000, 1), Connection::Live);
    assert_eq!(radio.connects(peers[0]), 2);
}

#[test]
fn unreadable_saved_record_is_kept_and_reported() {
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime
        .load_preferences(&firmware_shell::preferences::AppPreferences::new(&[3, 1, 2]).unwrap());
    let mut radio = Radio::new();
    advance(&mut runtime, &mut radio, 0);
    assert_eq!(
        runtime.menu.diagnostics().notice,
        Some(MenuMessage::SavedUnreadable)
    );
    // Selections still work until restart, but are not written.
    runtime.request_sensor(
        crate::screens::sensors::Action::Connect(peer(HEART, 1)),
        &mut radio,
        100,
    );
    assert_eq!(radio.connects(peer(HEART, 1)), 1);
    assert_eq!(
        runtime.preferences.persistence(),
        super::super::preferences::Persistence::Locked
    );
}

#[test]
fn discovery_waits_for_a_search_in_progress_and_reconnects_after_it() {
    let heart = peer(HEART, 4242);
    let mut runtime = restored(&[heart]);
    let mut radio = Radio::quiet();
    run(&mut runtime, &mut radio, 0, 15_000);
    // The rider opens Sensors while the sensor is still missing.
    runtime.navigation.page = Page::Home;
    runtime.navigation.home_cursor = 1;
    runtime.input(press(Button::BottomRight), 15_000, &mut radio);
    run(&mut runtime, &mut radio, 15_000, 26_000);
    let scan = radio
        .requests
        .iter()
        .position(|op| matches!(op, AntOperation::Scan(_)))
        .expect("discovery started");
    // No search opened while discovery ran; reconnecting resumed after it.
    let connects_before = radio.requests[..scan]
        .iter()
        .filter(|op| matches!(op, AntOperation::Connect(_)))
        .count();
    run(&mut runtime, &mut radio, 26_000, 80_000);
    assert!(radio.connects(heart) > connects_before);
}

/// Save trouble stays on the page through discovery, which only ever
/// replaces its own progress messages.
#[test]
fn save_problems_stay_visible_while_discovery_runs() {
    let mut runtime = Runtime::new(Profile::HeartRate);
    runtime
        .load_preferences(&firmware_shell::preferences::AppPreferences::new(&[3, 1, 2]).unwrap());
    let mut radio = Radio::new();
    run(&mut runtime, &mut radio, 0, 1_000);
    // Opening Sensors starts a search whose progress replaces page messages.
    runtime.navigation.page = Page::Home;
    runtime.navigation.home_cursor = 1;
    runtime.input(press(Button::BottomRight), 1_000, &mut radio);
    for (from, to) in [(1_000, 1_500), (1_500, 4_000), (4_000, 20_000)] {
        run(&mut runtime, &mut radio, from, to);
        assert_eq!(
            runtime.menu.diagnostics().notice,
            Some(MenuMessage::SavedUnreadable),
            "at {to} ms"
        );
    }
    assert!(
        radio
            .requests
            .iter()
            .any(|op| matches!(op, AntOperation::Scan(_)))
    );

    // A failing save is reported through a search, and cleared once saved.
    let mut runtime = Runtime::new(Profile::HeartRate);
    let mut radio = Radio::new();
    run(&mut runtime, &mut radio, 0, 1_000);
    runtime.navigation.page = Page::Home;
    runtime.navigation.home_cursor = 1;
    runtime.input(press(Button::BottomRight), 1_000, &mut radio);
    run(&mut runtime, &mut radio, 1_000, 1_200);
    runtime.preferences.save_failed(1_200);
    run(&mut runtime, &mut radio, 1_200, 6_000);
    assert_eq!(
        runtime.menu.diagnostics().notice,
        Some(MenuMessage::NotSaved)
    );
    assert_ne!(
        runtime.menu.diagnostics().message,
        Some(MenuMessage::NotSaved)
    );
    runtime.preferences = super::super::preferences::Preferences::default();
    run(&mut runtime, &mut radio, 6_000, 6_100);
    assert_eq!(runtime.menu.diagnostics().notice, None);
}

/// Replacing a sensor whose old channel cannot be closed retries the close
/// with the same bounded backoff, never once per tick.
#[test]
fn replacement_backs_off_when_the_old_channel_keeps_failing_to_close() {
    let old = peer(HEART, 4242);
    let new = peer(HEART, 5151);
    let mut runtime = restored(&[old]);
    let mut radio = Radio::new();
    run(&mut runtime, &mut radio, 0, 100);
    assert_eq!(link(&radio, HEART, 100), Some(LinkState::Connected));
    radio.fail_disconnects = true;
    runtime.request_sensor(
        crate::screens::sensors::Action::Connect(new),
        &mut radio,
        100,
    );
    assert_eq!(link(&radio, HEART, 100), Some(LinkState::TransportLost));
    let closes = |radio: &Radio| {
        radio
            .requests
            .iter()
            .filter(|op| matches!(op, AntOperation::Disconnect(HEART)))
            .count()
    };
    run(&mut runtime, &mut radio, 100, 1_100);
    assert!(closes(&radio) <= 2, "{} closes in a second", closes(&radio));
    run(&mut runtime, &mut radio, 1_100, 120_000);
    // 1, 2, 4, 8, 16, then 30 s apart: a handful in two minutes.
    let attempts = closes(&radio);
    assert!((5..=10).contains(&attempts), "{attempts} closes");
    assert_eq!(radio.connects(new), 0);
    assert_eq!(
        connection(&runtime, &radio, 120_000, HEART),
        Connection::Replacing
    );

    // The next close succeeds and the replacement connects right after it.
    radio.fail_disconnects = false;
    let mut now = 120_000;
    while radio.connects(new) == 0 {
        assert!(now < 151_000, "replacement never connected");
        run(&mut runtime, &mut radio, now, now + 10);
        now += 10;
    }
    assert_eq!(link(&radio, HEART, now), Some(LinkState::Connected));
    assert_eq!(closes(&radio), attempts + 1);
}
