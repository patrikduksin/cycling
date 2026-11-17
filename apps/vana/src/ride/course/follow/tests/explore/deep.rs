use super::*;

#[test]
#[ignore = "tier 2: cycling verify --deep"]
fn off_course_on_the_straight() {
    let raises = [Event::OffCourse, Event::StillOff, Event::OnCourse];
    closes_raising(Bounds::off_course(), Some(Shape::Straight), &raises, &[]);
}

#[test]
#[ignore = "tier 2: cycling verify --deep"]
fn times_on_the_straight() {
    closes(Bounds::times(), Some(Shape::Straight));
}

#[test]
#[ignore = "tier 2: cycling verify --deep"]
fn times_on_the_l() {
    closes(Bounds::times(), Some(Shape::L));
}

#[test]
#[ignore = "tier 2: cycling verify --deep"]
fn places_on_the_hairpin() {
    closes(Bounds::places(), Some(Shape::Hairpin));
}

#[test]
#[ignore = "tier 2: cycling verify --deep"]
fn times_on_the_hairpin() {
    closes(Bounds::times(), Some(Shape::Hairpin));
}
