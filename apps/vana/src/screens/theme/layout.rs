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
        dense_row: 26,
        item: 46,
        badge: 46,
        button: 45,
        ring: 3,
        bar: 6,
        mark: 3,
        track: 14,
        stroke: 2,
        marker: 10,
        chart: 40,
        lead: 30,
        small_disc: 24,
    },
    radius: Radius {
        button: 14,
        card: 12,
        bar: 2,
    },
    face: Type {
        tag: FaceRole::Tag,
        hero: FaceRole::Hero,
        hero_narrow: FaceRole::Large,
        digits: FaceRole::Digits40,
        digits_small: FaceRole::Digits26,
        digits_mid: FaceRole::Digits34,
        percent_mid: FaceRole::Digits22,
        summary: FaceRole::Digits19,
        line: FaceRole::Digits17,
        heading: FaceRole::Heading,
        label: FaceRole::Label,
        note: FaceRole::Note,
        clock: FaceRole::Clock,
    },
};
