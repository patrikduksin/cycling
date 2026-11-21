//! The benchmark screens (every VANA page a rider reaches) paint the same
//! pixels: a faster painter must not change a frame. Hashes are from
//! `sim-bench`, which prints them. A change that means to alter one of these
//! screens updates its hash here and shows the new screen in its sim-compare.
use shell_simulator::bench::{SCENES, hash, measure};

const HASHES: [(&str, u64); 37] = [
    ("home", 0x5dea_9846_5bc7_40a5),
    ("ride main + radar", 0x55ca_baa8_acff_4d29),
    ("climb", 0xe75f_01b9_b523_841a),
    ("summary", 0xd622_b712_508c_02b9),
    ("sensors list", 0x0f56_378e_4d72_916a),
    ("workouts", 0x3fcb_3076_c488_efd3),
    ("ride stats", 0x00f8_599e_523a_5963),
    ("lap", 0xe706_6f5d_b71d_ef5e),
    ("workout", 0x0b85_0bc6_c134_558f),
    ("map", 0x461a_f018_c884_7685),
    ("map course", 0xe716_40ab_90e8_66e7),
    ("map off course", 0x2703_b1d8_b424_b034),
    ("map santiago 15 km/h", 0x03b7_f01f_6cab_d7a3),
    ("map santiago 25 km/h", 0xd75c_c9a0_4220_2310),
    ("map santiago 50 km/h", 0xcf98_21d1_1bb1_d87f),
    ("map santiago 90 km/h", 0x5fea_af4e_fc7e_c833),
    ("paused", 0xd866_a09e_c685_dad9),
    ("end ride", 0xf8c7_48ec_c101_21a5),
    ("ready", 0x3dc6_4149_a715_c58c),
    ("settings", 0x7b6c_3a58_d239_aba0),
    ("setup", 0x2a09_c578_4c67_1ae2),
    ("phone", 0x03bc_91b7_8e55_fe64),
    ("bests", 0x5604_82f0_1d93_a35b),
    ("form", 0x7fee_247f_8a94_0413),
    ("history", 0x8d63_f4e0_1f32_cb39),
    ("past ride", 0x60e1_07d6_6caf_d135),
    ("sensor detail", 0xb828_83a4_b1ed_292c),
    ("summary page 2", 0x9cda_d25e_d349_27a8),
    ("boot", 0x3181_4f05_3737_c050),
    ("setup power step", 0x4176_3293_5a0d_f513),
    ("settings editing", 0x1375_b9da_f552_f2b3),
    ("bests confirm", 0xf44d_b90b_3aa2_eb72),
    ("form no power", 0x02b3_0b8e_7e28_4f9d),
    ("phone from saved ride", 0xffb0_a2fb_7c21_fe75),
    ("main with workout band", 0x7b20_9ddf_6283_0128),
    ("settings light", 0x6249_16b5_eb56_c4f1),
    ("home light", 0xcc7d_013b_ca1e_1ceb),
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
