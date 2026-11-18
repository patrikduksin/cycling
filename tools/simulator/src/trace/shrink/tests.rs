use super::*;
use device_api::journal::MethodId;
use firmware_services::trace::Call;

fn call(capability: &str, name: &str) -> Event<'static> {
    let method: MethodId = crate::twin::script::named(capability, name).unwrap();
    Event::Call(Call {
        method,
        now_ms: 0,
        body: &[0],
    })
}

#[test]
fn a_repeat_and_the_observations_a_request_refreshed_go_with_it_and_media_stay() {
    let take = crate::twin::script::named("Ant", "take_packet").unwrap();
    let events = [
        Event::Pass { now_ms: 10 },
        call("Ant", "scanning"),
        call("Ant", "request"),
        call("Ant", "scanning"),
        call("Ant", "take_packet"),
        Event::Repeated {
            method: take,
            count: 3,
        },
        call("OwnedFlash", "read"),
        Event::Pass { now_ms: 20 },
        call("Ant", "scanning"),
    ];
    let Units {
        fixed,
        removable: units,
    } = records(&events);
    assert_eq!(fixed, [0, 6, 7]);
    assert_eq!(units, [vec![1], vec![2, 3], vec![4, 5], vec![8]]);
}

#[test]
fn the_boot_and_the_judged_pass_stay_and_each_pass_or_command_between_is_a_unit() {
    let events = [
        Event::PoweredOff,
        Event::Pass { now_ms: 10 },
        call("Ant", "scanning"),
        Event::Command("CMD 1 RIDE START"),
        call("Ant", "request"),
        Event::Pass { now_ms: 20 },
        call("Ant", "scanning"),
    ];
    let Units {
        fixed,
        removable: units,
    } = passes(&events);
    assert_eq!(fixed, [0, 5, 6]);
    assert_eq!(units, [vec![1, 2], vec![3, 4]]);
}

#[test]
fn outputs_go_in_one_try_and_every_record_a_replay_reads_stays() {
    let events = [
        Event::Pass { now_ms: 10 },
        Event::Page("home"),
        call("Ant", "scanning"),
        Event::Panel(7),
    ];
    let Units {
        fixed,
        removable: units,
    } = outputs(&events);
    assert_eq!(fixed, [0, 2]);
    assert_eq!(units, [vec![1, 3]]);
}
