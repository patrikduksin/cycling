pub mod alerts;
pub mod auto;
pub mod battery_life;
pub mod bests;
pub mod bulk;
pub mod climb;
pub mod climb_ahead;
pub mod course;
pub mod deleted;
pub mod directory;
pub mod elevation;
pub mod facts;
pub mod fit;
pub mod fitness;
pub mod gps_speed;
pub mod homing;
pub mod index;
pub mod journal;
pub mod laps;
pub mod load;
pub mod log;
pub mod metrics;
pub mod reclaim;
pub mod recorder;
pub mod recovery;
pub mod replay;
pub mod rider;
pub mod storage;
pub mod thin;
pub mod totals;
pub mod turn_back;
pub mod week;
pub mod wheel_size;
pub mod zones;

/// What a file VANA writes into FILES holds, as its owner reads it. Its owner
/// never writes over a `Foreign` file; only FORMAT replaces one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contents {
    Missing,
    Current,
    Damaged,
    Foreign,
}

pub(crate) fn another_version(head: &[u8], magic: &[u8; 4], version: u8) -> bool {
    head.len() > 4 && head[..4] == magic[..] && head[4] != version
}
