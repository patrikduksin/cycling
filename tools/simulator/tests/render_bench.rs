//! The benchmark screens (every VANA page a rider reaches) paint the same
//! pixels: a faster painter must not change a frame. Hashes are from
//! `sim-bench`, which prints them. A change that means to alter one of these
//! screens updates its hash here and shows the new screen in its sim-compare.
#![cfg(feature = "cycling")]
use shell_simulator::bench::{SCENES, hash, measure};

const HASHES: [(&str, u64); 32] = [
    ("home", 0x0c91_c4e2_1f97_76ab),
    ("ride main + radar", 0x88f1_eb47_3bc0_b638),
    ("climb", 0x2632_088b_5b17_44fa),
    ("summary", 0x7297_e332_ce5a_8371),
    ("sensors list", 0x294c_50dd_fdb8_bbf1),
    ("workouts", 0x3fcb_3076_c488_efd3),
    ("ride stats", 0xc10e_4c0f_0d66_f3cf),
    ("lap", 0x9bef_c4b9_62e2_d112),
    ("workout", 0xfdbe_faf1_1f21_0d3c),
    ("map", 0x88cd_b2a3_fd8e_de60),
    ("back to start", 0x1920_6d8e_0f5d_1fe3),
    ("paused", 0x1abb_4ec0_be41_8109),
    ("end ride", 0x45ab_e5fa_c73d_1778),
    ("ready", 0x3dc6_4149_a715_c58c),
    ("settings", 0x7b6c_3a58_d239_aba0),
    ("setup", 0x2a09_c578_4c67_1ae2),
    ("phone", 0x03bc_91b7_8e55_fe64),
    ("bests", 0x5942_c60f_15ca_7f9b),
    ("form", 0x0470_d411_bce3_f3f3),
    ("history", 0x5754_10a7_c14c_4d01),
    ("past ride", 0x62e4_4c6e_87bf_f7b5),
    ("sensor detail", 0xa211_02a4_cc4d_c92e),
    ("summary page 2", 0x547f_b158_ece4_58aa),
    ("boot", 0x3181_4f05_3737_c050),
    ("setup power step", 0x4176_3293_5a0d_f513),
    ("settings editing", 0x1375_b9da_f552_f2b3),
    ("bests confirm", 0x40ab_8b32_d5ef_28e6),
    ("form no power", 0x5ed1_0eab_524c_8dc5),
    ("phone from saved ride", 0xffb0_a2fb_7c21_fe75),
    ("main with workout band", 0x0633_e69a_fe9f_b67b),
    ("settings light", 0x59c2_096e_5dc3_6e44),
    ("home light", 0x9370_23ef_3b06_01bf),
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
