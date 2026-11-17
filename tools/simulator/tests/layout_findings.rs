use shell_simulator::bench::SCENES;

#[test]
#[ignore = "a report, not a gate"]
fn canvas_screens_report_what_the_layout_check_finds() {
    let mut found = Vec::new();
    for scene in &SCENES {
        vana::screens::check::record();
        let _sim = (scene.reach)();
        found.extend(vana::screens::check::findings());
    }
    found.sort();
    found.dedup();
    for finding in &found {
        println!("{finding}");
    }
    println!("{} findings", found.len());
}
