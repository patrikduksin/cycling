//! Gaps, radii and faces, in scaled units, that both looks share.
use super::{Layout, Radius, Space, Type};
use firmware_shell::rendering::font::FaceRole;

pub const LAYOUT: Layout = Layout {
    space: Space {
        gutter: 10,
        pad: 8,
        gap: 6,
        tight: 4,
        rule: 1,
        top_line: 24,
        dot: 8,
        row: 30,
        button: 45,
        ring: 3,
        bar: 6,
        mark: 3,
        track: 14,
    },
    radius: Radius {
        button: 14,
        card: 12,
        bar: 2,
    },
    face: Type {
        tag: FaceRole::Tag,
        hero: FaceRole::Hero,
        digits: FaceRole::Digits40,
        digits_small: FaceRole::Digits26,
        line: FaceRole::Digits15,
        heading: FaceRole::Heading,
        label: FaceRole::Label,
        note: FaceRole::Note,
    },
};
