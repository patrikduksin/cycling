use super::*;

fn method(capability: &str, name: &str) -> MethodId {
    MethodId::named(capability, name).unwrap()
}

fn connect() -> MethodId {
    method("Ant", "request")
}

fn play() -> MethodId {
    method("Sound", "play")
}

fn packet() -> MethodId {
    method("Ant", "take_packet")
}

fn channels() -> MethodId {
    method("Ant", "channels")
}

fn called(method: MethodId, args: &[u8], outcome: &[u8]) -> Record {
    Record::Called {
        method,
        args: args.to_vec(),
        outcome: outcome.to_vec(),
    }
}

fn observed(method: MethodId, value: &[u8]) -> Record {
    Record::Observed(Observed {
        method,
        key: Vec::new(),
        value: value.to_vec(),
    })
}

fn identity(method: MethodId, args: &[u8], occurrence: u32, epoch: u32) -> Identity {
    Identity {
        method,
        args: args.to_vec(),
        occurrence,
        epoch,
    }
}

fn recorded() -> Pass {
    Pass::new(
        1_000,
        View::default(),
        [
            observed(channels(), b"closed"),
            called(connect(), &[1], b"ok"),
            observed(channels(), b"open"),
            called(play(), &[7], b"played"),
            called(packet(), &[], b"first"),
            called(packet(), &[], b"second"),
        ],
    )
}

#[test]
fn the_calls_the_device_made_replay_and_the_epoch_advances_when_an_answer_matches() {
    let mut pass = recorded();
    assert_eq!(
        pass.observe(channels(), &[]).map(|seen| &seen.value[..]),
        Some(&b"closed"[..])
    );
    assert_eq!(pass.answer(connect(), &[1]), Ok(b"ok".to_vec()));
    assert_eq!(
        pass.observe(channels(), &[]).map(|seen| &seen.value[..]),
        Some(&b"open"[..])
    );
    assert_eq!(pass.answer(play(), &[7]), Ok(b"played".to_vec()));
    assert_eq!(pass.poll(packet()), Some(b"first".to_vec()));
    assert_eq!(pass.poll(packet()), Some(b"second".to_vec()));
    assert_eq!(pass.end().map(drop), Ok(()));
}

#[test]
fn a_request_the_twin_never_makes_is_unconsumed_at_the_end_of_the_pass() {
    let mut pass = recorded();
    assert_eq!(pass.answer(connect(), &[1]), Ok(b"ok".to_vec()));
    pass.poll(packet());
    pass.poll(packet());
    assert_eq!(
        pass.end(),
        Err(Stop::Unconsumed(vec![identity(play(), &[7], 0, 0)]))
    );
}

#[test]
fn a_request_made_twice_has_no_answer_the_second_time() {
    let mut pass = recorded();
    assert_eq!(pass.answer(play(), &[7]), Ok(b"played".to_vec()));
    let again = pass.answer(play(), &[7]);
    assert_eq!(again, Err(Stop::Unanswered(identity(play(), &[7], 1, 1))));
    assert_eq!(
        again.unwrap_err().to_string(),
        "left the recorded world: Sound::play(07) #1 in epoch 1 has no answer"
    );
}

#[test]
fn a_request_delayed_past_another_capabilitys_refresh_still_replays() {
    let mut pass = Pass::new(
        1_000,
        View::default(),
        [
            called(play(), &[7], b"played"),
            called(connect(), &[1], b"ok"),
        ],
    );
    assert_eq!(pass.answer(connect(), &[1]), Ok(b"ok".to_vec()));
    assert_eq!(pass.answer(play(), &[7]), Ok(b"played".to_vec()));
    assert_eq!(pass.end().map(drop), Ok(()));
}

#[test]
fn a_request_delayed_past_its_own_capabilitys_refresh_is_unanswered() {
    let mut pass = Pass::new(
        1_000,
        View::default(),
        [
            called(connect(), &[1], b"ok"),
            called(connect(), &[2], b"ok"),
        ],
    );
    assert_eq!(
        pass.answer(connect(), &[2]),
        Err(Stop::Unanswered(identity(connect(), &[2], 0, 0)))
    );
}

#[test]
fn a_request_delayed_to_the_next_pass_is_unconsumed_in_its_own() {
    let recorded = || [called(play(), &[7], b"played")];
    let (first, mut next) = (
        Pass::new(1_000, View::default(), recorded()),
        Pass::new(1_000, View::default(), []),
    );
    assert_eq!(
        first.end(),
        Err(Stop::Unconsumed(vec![identity(play(), &[7], 0, 0)]))
    );
    assert_eq!(
        next.answer(play(), &[7]),
        Err(Stop::Unanswered(identity(play(), &[7], 0, 0)))
    );
}

#[test]
fn takes_reordered_with_another_capabilitys_request_replay() {
    let mut pass = recorded();
    assert_eq!(pass.answer(connect(), &[1]), Ok(b"ok".to_vec()));
    assert_eq!(pass.poll(packet()), Some(b"first".to_vec()));
    assert_eq!(pass.answer(play(), &[7]), Ok(b"played".to_vec()));
    assert_eq!(pass.poll(packet()), Some(b"second".to_vec()));
    assert_eq!(pass.end().map(drop), Ok(()));
}

#[test]
fn a_take_recorded_after_its_capabilitys_refresh_is_not_served_before_it() {
    let mut pass = recorded();
    assert_eq!(pass.poll(packet()), None);
    assert_eq!(pass.answer(connect(), &[1]), Ok(b"ok".to_vec()));
    assert_eq!(pass.poll(packet()), Some(b"first".to_vec()));
}

#[test]
fn polls_the_device_found_empty_are_not_recorded_and_more_or_fewer_of_them_replay() {
    let mut pass = Pass::new(1_000, View::default(), [called(packet(), &[], b"only")]);
    assert_eq!(pass.poll(packet()), Some(b"only".to_vec()));
    assert_eq!(pass.poll(packet()), None);
    assert_eq!(pass.poll(packet()), None);
    assert_eq!(pass.end().map(drop), Ok(()));
    assert_eq!(Pass::new(1_000, View::default(), []).poll(packet()), None);
}

#[test]
fn a_take_that_must_answer_and_has_no_record_is_unanswered() {
    let entropy = method("ReadWrite", "entropy");
    let mut pass = Pass::new(1_000, View::default(), []);
    assert_eq!(
        pass.answer(entropy, &[]),
        Err(Stop::Unanswered(identity(entropy, &[], 0, 0)))
    );
}

#[test]
fn an_observation_the_twin_has_not_reached_is_not_served() {
    let pass = Pass::new(
        1_000,
        View::default(),
        [
            called(connect(), &[1], b"ok"),
            observed(channels(), b"open"),
        ],
    );
    assert_eq!(pass.observe(channels(), &[]), None);
}

#[test]
fn items_the_twin_polls_for_fewer_times_than_the_device_are_unconsumed() {
    let mut pass = recorded();
    assert_eq!(pass.answer(connect(), &[1]), Ok(b"ok".to_vec()));
    assert_eq!(pass.answer(play(), &[7]), Ok(b"played".to_vec()));
    assert_eq!(pass.poll(packet()), Some(b"first".to_vec()));
    assert_eq!(
        pass.end(),
        Err(Stop::Unconsumed(vec![identity(packet(), &[], 1, 1)]))
    );
}

#[test]
fn an_observation_that_did_not_change_is_served_in_the_next_pass_with_the_time_it_was_read() {
    let mut first = Pass::new(
        1_000,
        View::default(),
        [
            called(connect(), &[1], b"ok"),
            observed(channels(), b"open"),
        ],
    );
    assert_eq!(first.answer(connect(), &[1]), Ok(b"ok".to_vec()));
    let next = Pass::new(2_000, first.end().unwrap(), []);
    assert_eq!(
        next.observe(channels(), &[]),
        Some(&Seen {
            value: b"open".to_vec(),
            at_ms: 1_000,
        }),
        "a carried value keeps the pass it was read at, to age from"
    );
}

#[test]
fn an_observation_rides_with_its_capabilitys_refresh_not_the_call_before_it() {
    let status = method("Ant", "send_status");
    let mut pass = Pass::new(
        1_000,
        View::default(),
        [
            called(connect(), &[1], b"ok"),
            called(packet(), &[], b"taken"),
            called(play(), &[7], b"played"),
            Record::Observed(Observed {
                method: status,
                key: vec![3],
                value: b"sent".to_vec(),
            }),
        ],
    );
    assert_eq!(pass.answer(connect(), &[1]), Ok(b"ok".to_vec()));
    assert_eq!(
        pass.observe(status, &[3]).map(|seen| &seen.value[..]),
        Some(&b"sent"[..])
    );
}

#[test]
fn calls_left_unconsumed_are_named_in_the_order_the_device_made_them() {
    let pass = Pass::new(
        1_000,
        View::default(),
        [
            called(play(), &[7], b"played"),
            called(connect(), &[1], b"ok"),
        ],
    );
    assert_eq!(
        pass.end(),
        Err(Stop::Unconsumed(vec![
            identity(play(), &[7], 0, 0),
            identity(connect(), &[1], 0, 0),
        ]))
    );
}
