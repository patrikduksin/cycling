//! The benchmark screens (every VANA page a rider reaches) paint the same
//! pixels: a faster painter must not change a frame. Hashes are from
//! `sim-bench`, which prints them. A change that means to alter one of these
//! screens updates its hash here and shows the new screen in its sim-compare.
use shell_simulator::bench::{SCENES, hash, measure};

const HASHES: [(&str, u64); 20] = [
    ("home", 0xb999_9673_cc95_c2ad),
    ("ride main + radar", 0x85e7_b83b_5114_637a),
    ("climb", 0x006e_0bbb_28e9_29c1),
    ("ride done", 0x9cc7_2ed6_69b8_b55d),
    ("sensors list", 0x328d_b813_7dee_22a5),
    ("ride stats", 0xb1e5_83a4_9246_782b),
    ("workout", 0x6712_77b3_69b1_f236),
    ("map", 0x37c2_7857_c6cb_e81b),
    ("map course", 0x5cb4_7234_acbc_af0a),
    ("map off course", 0xa7fb_126c_606e_08bb),
    ("map santiago 15 km/h", 0xbeb5_76b0_752d_a326),
    ("map santiago 25 km/h", 0x25d9_5f64_e0ac_dbb2),
    ("map santiago 50 km/h", 0x24d5_2f25_91bf_eb0f),
    ("map santiago 90 km/h", 0x24d9_02df_f101_df6c),
    ("paused", 0x762c_8bfe_f4f3_593d),
    ("end ride", 0x842f_6fb4_9c1e_aad9),
    ("sensor detail", 0xb3e2_caaf_1260_c5e9),
    ("boot", 0x8099_3d2c_de83_6750),
    ("main with workout band", 0x7c8e_53c8_1f62_0ce6),
    ("home light", 0x4936_5942_5094_f847),
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
