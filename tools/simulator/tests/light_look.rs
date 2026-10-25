//! The light look maps every painted pixel on its way to the panel (#219);
//! the screen reader reads what that shows (#295).
#![cfg(feature = "cycling")]
use device_api::input::Button;
use shell_simulator::bench::SCENES;
use shell_simulator::sim::{Sim, seconds};

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
