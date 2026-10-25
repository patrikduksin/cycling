//! The light look maps every painted pixel on its way to the panel (#219);
//! the screen reader reads what that shows (#295).
#![cfg(feature = "cycling")]
use device_api::input::Button;
use shell_simulator::bench::SCENES;
use shell_simulator::screen::Look;
use shell_simulator::sim::{Sim, seconds};
use std::sync::LazyLock;
use vana::screens::look;

fn reach(name: &str) -> Sim {
    let scene = SCENES.iter().find(|scene| scene.name == name).unwrap();
    (scene.reach)()
}

#[test]
fn reads_home_settings_and_a_ride_page_in_the_light_look() {
    let mut sim = reach("settings light");
    assert_eq!(
        sim.screen().text(),
        "7:00\nGPS\nDISPLAY\nBRIGHTNESS\n50%\nDIM AFTER\n30 S\nDAY / NIGHT\nAUTO\nDAY NOW\nTHEME\nLIGHT\nSAVING"
    );
    sim.press(Button::TopLeft);
    assert_eq!(
        sim.screen().text(),
        "7:00\nGPS\nRIDE\nGPS READY\nREADY TO RIDE\nSENSORS\n3/3 LIVE\nHR\nPWR\nSPD/C\nTRAIN\nSETTINGS\nRIDES\nNONE YET"
    );
    sim.start_ride();
    sim.ride_at(28.0, 230.0, 142.0, 88.0);
    sim.advance(seconds(20));
    assert_eq!(
        sim.screen().text(),
        "7:00\nREC\nGPS\n0:00:20\n2.4%\n27.9\nKM/H\n230\nZ5\nW\n142\nZ1\nBPM\n88\n0.15\nRPM\nKM"
    );
}

/// Every dark benchmark screen, mapped through the light look as VANA maps
/// it, reads the same text in the colours the look shows.
#[test]
fn every_dark_screen_reads_the_same_through_the_light_look() {
    static LIGHT: LazyLock<Look> = LazyLock::new(|| Look::new(look::light));
    for scene in SCENES.iter().filter(|scene| !scene.name.contains("light")) {
        let dark = (scene.reach)().screen();
        assert!(dark.look.is_none(), "{}", scene.name);
        let mut shown = dark.clone();
        look::apply(&mut shown.pixels);
        shown.look = Some(&LIGHT);
        let expected: Vec<_> = dark
            .frame()
            .read()
            .into_iter()
            .map(|run| (run.text, run.x, run.y, look::light(run.color)))
            .collect();
        let read: Vec<_> = shown
            .frame()
            .read()
            .into_iter()
            .map(|run| (run.text, run.x, run.y, run.color))
            .collect();
        assert_eq!(read, expected, "{}", scene.name);
    }
}
