//! The benchmark screens (every VANA page a rider reaches) paint the same
//! pixels: a faster painter must not change a frame. Hashes are from
//! `sim-bench`, which prints them. A change that means to alter one of these
//! screens updates its hash here and shows the new screen in its sim-compare.
use shell_simulator::bench::{SCENES, hash, measure};

const HASHES: [(&str, u64); 20] = [
    ("home", 0x0429_4bb6_6cb7_3a62),
    ("ride main + radar", 0xe7a7_1d79_f420_f5fe),
    ("climb", 0x5932_93af_d54f_a42c),
    ("ride done", 0x0685_ae2f_255b_93e7),
    ("sensors list", 0x2040_cc86_31ed_0b9f),
    ("ride stats", 0x2ff9_d8f2_6966_aacd),
    ("workout", 0xa6a4_1c2a_4d42_4731),
    ("map", 0x2b3c_2e35_51b5_af30),
    ("map course", 0x0278_0a3f_9907_f761),
    ("map off course", 0xd594_2425_1369_c754),
    ("map santiago 15 km/h", 0xda20_2685_bb68_2c6c),
    ("map santiago 25 km/h", 0xaf44_4795_3ab0_6e4d),
    ("map santiago 50 km/h", 0x8c15_7648_03ab_a5a2),
    ("map santiago 90 km/h", 0xddc8_58ea_3b96_954d),
    ("paused", 0xb421_9bb3_ce77_13d8),
    ("end ride", 0x3928_7ad5_faf3_965c),
    ("sensor detail", 0x91e4_9888_187d_fca8),
    ("boot", 0x8099_3d2c_de83_6750),
    ("main with workout band", 0xccdb_69f3_c41b_e657),
    ("home light", 0xc73e_975f_8bfa_68b7),
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
