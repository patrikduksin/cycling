//! The benchmark screens (every VANA page a rider reaches) paint the same
//! pixels: a faster painter must not change a frame. Hashes are from
//! `sim-bench`, which prints them. A change that means to alter one of these
//! screens updates its hash here and shows the new screen in its sim-compare.
use shell_simulator::bench::{SCENES, hash, measure};

const HASHES: [(&str, u64); 25] = [
    ("home", 0x9b65_c3d8_8917_03c8),
    ("ride main + radar", 0x1dcd_5a09_16e2_7ae2),
    ("climb", 0xe292_a776_61df_eae8),
    ("summary", 0x5a0e_ebe1_49e6_6dc5),
    ("sensors list", 0x9adb_bd53_c833_b67d),
    ("workouts", 0x7832_b3fd_9df0_0bc3),
    ("ride stats", 0xb1e5_83a4_9246_782b),
    ("workout", 0x6712_77b3_69b1_f236),
    ("map", 0x37c2_7857_c6cb_e81b),
    ("map course", 0x0cff_3534_7556_b5d1),
    ("map off course", 0xa7fb_126c_606e_08bb),
    ("map santiago 15 km/h", 0xe2ca_2566_4cd6_2db0),
    ("map santiago 25 km/h", 0x8656_8472_e442_d64e),
    ("map santiago 50 km/h", 0x6c6a_1533_1183_136f),
    ("map santiago 90 km/h", 0x98cd_079a_0e28_4cea),
    ("paused", 0x60ff_a180_4e97_e5b5),
    ("end ride", 0x4505_1d0a_644b_2896),
    ("settings", 0x4f04_dc7f_2016_6b9e),
    ("sensor detail", 0xf81f_6852_8438_1b56),
    ("summary page 2", 0x2bea_3638_de11_ae79),
    ("boot", 0x8099_3d2c_de83_6750),
    ("settings editing", 0x020e_1ae4_9ad4_27b3),
    ("main with workout band", 0x74bd_c70d_3585_bc6d),
    ("settings light", 0x6f90_3724_84f3_bde8),
    ("home light", 0xcd4e_ca43_ee2d_3c20),
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
