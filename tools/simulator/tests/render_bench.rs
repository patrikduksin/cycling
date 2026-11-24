//! The benchmark screens (every VANA page a rider reaches) paint the same
//! pixels: a faster painter must not change a frame. Hashes are from
//! `sim-bench`, which prints them. A change that means to alter one of these
//! screens updates its hash here and shows the new screen in its sim-compare.
use shell_simulator::bench::{SCENES, hash, measure};

const HASHES: [(&str, u64); 37] = [
    ("home", 0xb6e8_d872_50b4_6c01),
    ("ride main + radar", 0x5057_f35c_442f_8fc8),
    ("climb", 0xe75f_01b9_b523_841a),
    ("summary", 0x4f07_bd54_3572_83eb),
    ("sensors list", 0x9adb_bd53_c833_b67d),
    ("workouts", 0x7832_b3fd_9df0_0bc3),
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
    ("paused", 0x60ff_a180_4e97_e5b5),
    ("end ride", 0xacc9_f40b_cde4_f956),
    ("ready", 0xeb81_968b_4a8f_1dfd),
    ("settings", 0x690b_e217_2f2c_0b45),
    ("setup", 0x6e45_dba2_551d_0162),
    ("phone", 0x1bd6_f259_531f_f6e4),
    ("bests", 0x5604_82f0_1d93_a35b),
    ("form", 0x7fee_247f_8a94_0413),
    ("history", 0xc933_3c05_e99f_0fda),
    ("past ride", 0xb829_cf70_3dc3_0888),
    ("sensor detail", 0xf81f_6852_8438_1b56),
    ("summary page 2", 0x912b_fc2f_eb34_8926),
    ("boot", 0x8099_3d2c_de83_6750),
    ("setup power step", 0x222d_7a04_3d8e_e393),
    ("settings editing", 0x020e_1ae4_9ad4_27b3),
    ("bests confirm", 0xf296_efcd_cd43_148d),
    ("form no power", 0x02b3_0b8e_7e28_4f9d),
    ("phone from saved ride", 0xf9a9_84da_7d2d_c3f5),
    ("main with workout band", 0x7b20_9ddf_6283_0128),
    ("settings light", 0x6f90_3724_84f3_bde8),
    ("home light", 0x745b_0928_c7f8_277f),
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
