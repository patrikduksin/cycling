//! The benchmark screens (every VANA page a rider reaches) paint the same
//! pixels: a faster painter must not change a frame. Hashes are from
//! `sim-bench`, which prints them. A change that means to alter one of these
//! screens updates its hash here and shows the new screen in its sim-compare.
use shell_simulator::bench::{SCENES, hash, measure};

const HASHES: [(&str, u64); 32] = [
    ("home", 0x5dea_9846_5bc7_40a5),
    ("ride main + radar", 0xf365_f621_41fa_2a0d),
    ("climb", 0x9329_f06e_2631_606d),
    ("summary", 0x6404_f2e0_1f19_a7b9),
    ("sensors list", 0x0f56_378e_4d72_916a),
    ("workouts", 0x3fcb_3076_c488_efd3),
    ("ride stats", 0x38c8_5703_1839_7873),
    ("lap", 0x9bef_c4b9_62e2_d112),
    ("workout", 0x75b3_e80b_c86a_0848),
    ("map", 0x19c2_6ffd_d6ac_5393),
    ("back to start", 0x1920_6d8e_0f5d_1fe3),
    ("paused", 0x1abb_4ec0_be41_8109),
    ("end ride", 0x45ab_e5fa_c73d_1778),
    ("ready", 0x3dc6_4149_a715_c58c),
    ("settings", 0x7b6c_3a58_d239_aba0),
    ("setup", 0x2a09_c578_4c67_1ae2),
    ("phone", 0x03bc_91b7_8e55_fe64),
    ("bests", 0x5942_c60f_15ca_7f9b),
    ("form", 0x0470_d411_bce3_f3f3),
    ("history", 0x8d63_f4e0_1f32_cb39),
    ("past ride", 0x62e4_4c6e_87bf_f7b5),
    ("sensor detail", 0xb828_83a4_b1ed_292c),
    ("summary page 2", 0xbd13_f974_c471_6f8c),
    ("boot", 0x3181_4f05_3737_c050),
    ("setup power step", 0x4176_3293_5a0d_f513),
    ("settings editing", 0x1375_b9da_f552_f2b3),
    ("bests confirm", 0x40ab_8b32_d5ef_28e6),
    ("form no power", 0x5ed1_0eab_524c_8dc5),
    ("phone from saved ride", 0xffb0_a2fb_7c21_fe75),
    ("main with workout band", 0x98ed_6b1c_e526_7bcc),
    ("settings light", 0x59c2_096e_5dc3_6e44),
    ("home light", 0x09fb_425c_a4c6_e514),
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
