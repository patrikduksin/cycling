//! The benchmark screens paint the same pixels: a faster painter must not
//! change a frame. Hashes are from `sim-bench`, which prints them. A change
//! that means to alter one of these screens updates its hash here and shows
//! the new screen in its sim-compare.
#![cfg(feature = "cycling")]
use shell_simulator::bench::{SCENES, hash, measure};

const HASHES: [(&str, u64); 6] = [
    ("home", 0x0c91_c4e2_1f97_76ab),
    ("ride main + radar", 0xc59f_cb1c_1dc8_1d87),
    ("climb", 0xfebc_a956_d7a5_0792),
    ("summary", 0x7297_e332_ce5a_8371),
    ("sensors list", 0x294c_50dd_fdb8_bbf1),
    ("workouts", 0xcf72_96ce_aab4_afee),
];

#[test]
fn benchmark_screens_paint_their_recorded_pixels() {
    assert_eq!(SCENES.len(), HASHES.len());
    for (scene, (name, recorded)) in SCENES.iter().zip(HASHES) {
        assert_eq!(scene.name, name);
        let mut sim = (scene.reach)();
        let request = measure(&mut sim, 2);
        assert!(request.stable, "{name}: repainting changed the frame");
        assert!(request.work.spans > 0, "{name}: nothing was counted");
        assert_eq!(
            hash(&sim.screen().pixels),
            request.hash,
            "{name}: the shown frame is the measured one"
        );
        assert_eq!(request.hash, recorded, "{name}: {:016x}", request.hash);
    }
}
