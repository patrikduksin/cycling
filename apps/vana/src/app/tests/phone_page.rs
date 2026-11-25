use super::phone::{Phone, Radio2, Transport, attach, saved_ride};
use super::presentation::Screen;
use super::*;
use crate::app::navigation::To;
use crate::app::phone::page::{CODE_TIMEOUT_MS, TICK_MS};
use crate::app::phone::{IDLE_MS, REOPEN_MS};
use crate::compose::builtin::ScreenId;
use crate::compose::input::OnPage;
use crate::screens::workout::Page;
use device_api::server::{Listen, Pairing, PhoneLink};
use std::string::String;

const OLD: Pairing = Pairing {
    id: [0xc6, 0x06, 0x0a, 0x01],
    key: [0x5a; 32],
};

fn phone<'a>(runtime: &'a mut Runtime, journal: &'a mut Journal, now: u64) -> Phone<'a> {
    attach(runtime);
    let mut transport = Transport::default();
    transport.link = PhoneLink::Advertising;
    let mut phone = Phone {
        runtime,
        journal,
        transport,
        radio: Radio2::default(),
        now,
    };
    phone.step();
    phone
}

fn pass(phone: &mut Phone, screen: &mut Screen) {
    phone.runtime.tick_phone_page(phone.now);
    phone.step();
    phone.runtime.save_pairing(&mut screen.shell);
}

#[test]
fn unpaired_the_code_s_pairing_seals_the_link_and_a_completed_handshake_saves_it() {
    let mut journal = Journal::erased();
    let (mut runtime, _, now) = saved_ride(&mut journal);
    let mut screen = Screen::saving();
    let mut phone = phone(&mut runtime, &mut journal, now);
    assert_eq!(
        phone.runtime.sealing(),
        None,
        "unpaired, nothing advertises"
    );
    assert_eq!(phone.transport.listening, Listen::Off);

    phone.runtime.open_phone_page(phone.now);
    assert_eq!(phone.runtime.navigation.page(), Page::Phone);
    assert_eq!(phone.runtime.screen_id(), ScreenId::PhoneCode);
    let pending = phone
        .runtime
        .sealing()
        .expect("the code's pairing seals the link");
    pass(&mut phone, &mut screen);
    assert_eq!(
        phone.transport.listening,
        Listen::Bluetooth,
        "it advertises the code's id"
    );
    assert_eq!(screen.shell.settings.pairing, None, "nothing is saved yet");
    assert!(screen.shell.full_light(), "the code is up at full light");

    phone.transport.link = PhoneLink::Connected;
    pass(&mut phone, &mut screen);
    assert_eq!(phone.runtime.screen_id(), ScreenId::PhoneDone);
    assert_eq!(screen.shell.settings.pairing, Some(pending));
    assert_eq!(phone.runtime.sealing(), Some(pending));
    assert_eq!(
        phone.runtime.phone_closed(),
        None,
        "the phone that paired stays on"
    );
    assert!(!screen.shell.full_light());

    phone.now += TICK_MS;
    pass(&mut phone, &mut screen);
    assert_eq!(
        phone.runtime.navigation.page(),
        Page::Home,
        "the tick closes itself"
    );
    assert_eq!(phone.transport.listening, Listen::Bluetooth);
}

#[test]
fn back_the_timeout_and_another_page_keep_the_old_pairing_and_its_link_returns() {
    let mut journal = Journal::erased();
    let (mut runtime, _, now) = saved_ride(&mut journal);
    let mut screen = Screen::saving();
    let mut phone = phone(&mut runtime, &mut journal, now);
    assert!(screen.shell.save_pairing(Some(OLD)));
    pass(&mut phone, &mut screen);
    phone.now += REOPEN_MS;
    pass(&mut phone, &mut screen);
    assert_eq!(phone.transport.listening, Listen::Bluetooth);

    enum Leave {
        Back,
        Timeout,
        ToSensors,
    }
    for leave in [Leave::Back, Leave::Timeout, Leave::ToSensors] {
        phone.runtime.open_phone_page(phone.now);
        assert_eq!(phone.runtime.screen_id(), ScreenId::PhonePaired);
        phone.runtime.phone_page_act(OnPage::Pair, phone.now);
        assert_eq!(phone.runtime.screen_id(), ScreenId::PhoneCode, "Pair new");
        let pending = phone.runtime.sealing().unwrap();
        assert_ne!(pending, OLD);
        pass(&mut phone, &mut screen);
        assert_eq!(phone.transport.listening, Listen::Off, "the old link drops");
        assert!(screen.shell.full_light());
        let left_to = match leave {
            Leave::Back => {
                phone.runtime.phone_page_act(OnPage::Back, phone.now);
                Page::Home
            }
            Leave::Timeout => {
                phone.now += CODE_TIMEOUT_MS;
                Page::Home
            }
            Leave::ToSensors => {
                phone.runtime.navigation.go(To::Page(Page::Sensors));
                Page::Sensors
            }
        };
        pass(&mut phone, &mut screen);
        assert_eq!(phone.runtime.navigation.page(), left_to);
        assert!(!screen.shell.full_light(), "the backlight is restored");
        assert_eq!(phone.runtime.sealing(), Some(OLD));
        assert_eq!(screen.shell.settings.pairing, Some(OLD));
        phone.now += REOPEN_MS;
        pass(&mut phone, &mut screen);
        assert_eq!(
            phone.transport.listening,
            Listen::Bluetooth,
            "and reconnects"
        );
        phone.transport.link = PhoneLink::Connected;
        pass(&mut phone, &mut screen);
        assert_eq!(screen.shell.settings.pairing, Some(OLD));
        phone.transport.link = PhoneLink::Advertising;
        pass(&mut phone, &mut screen);
        phone.now += REOPEN_MS;
        pass(&mut phone, &mut screen);
    }
}

#[test]
fn forget_deletes_the_pairing_and_the_device_stops_advertising() {
    let mut journal = Journal::erased();
    let (mut runtime, _, now) = saved_ride(&mut journal);
    let mut screen = Screen::saving();
    let mut phone = phone(&mut runtime, &mut journal, now);
    assert!(screen.shell.save_pairing(Some(OLD)));
    pass(&mut phone, &mut screen);
    phone.now += REOPEN_MS;
    pass(&mut phone, &mut screen);
    phone.runtime.open_phone_page(phone.now);
    phone.runtime.phone_page_act(OnPage::Forget, phone.now);
    assert_eq!(phone.runtime.screen_id(), ScreenId::ForgetPhone);
    phone
        .runtime
        .phone_page_act(OnPage::Answer(true), phone.now);
    pass(&mut phone, &mut screen);
    assert_eq!(screen.shell.settings.pairing, None);
    assert_eq!(phone.runtime.sealing(), None);
    assert_eq!(phone.runtime.navigation.page(), Page::Home);
    phone.now += IDLE_MS;
    pass(&mut phone, &mut screen);
    assert_eq!(phone.transport.listening, Listen::Off, "nothing advertises");
}

#[test]
fn phone_code_opens_the_code_and_replies_the_url_it_encodes() {
    let mut journal = Journal::erased();
    let (mut runtime, _, now) = saved_ride(&mut journal);
    let phone = phone(&mut runtime, &mut journal, now);
    let mut reply = String::new();
    assert_eq!(
        phone.runtime.code_command(phone.now, &mut reply),
        "ACCEPTED"
    );
    let pending = phone.runtime.sealing().unwrap();
    let id: String = pending
        .id
        .iter()
        .map(|byte| std::format!("{byte:02x}"))
        .collect();
    let url = crate::app::phone::pairing_url(&pending);
    assert_eq!(reply, std::format!("id={id} url={url}"));
    assert_eq!(phone.runtime.screen_id(), ScreenId::PhoneCode);
    let mut status = String::new();
    phone.runtime.write_paired(&mut status).unwrap();
    assert_eq!(status, std::format!(" code={id} paired=none"));
}
