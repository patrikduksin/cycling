//! Uptime milestones of turning on and riding, for `RIDE TIMING` and the
//! `cycling startup-timing` harness. Each first occurrence is kept for the
//! boot; ride-start and press-to-frame figures follow the latest ride.
use core::fmt::Write;

/// Device types (ANT+ numbers, or the `BLE_*` kinds below) whose first live
/// data this boot is remembered: one per ANT channel plus both BLE kinds.
/// More distinct kinds in one boot (channels reused for other types) are
/// counted in `sensors_dropped` rather than lost silently.
const SENSORS: usize = device_api::ant::CHANNEL_CAPACITY + 2;
/// A BLE heart-rate strap. Not an ANT+ device type.
pub(super) const BLE_HEART: u8 = 0;
/// A BLE cadence sensor. Not an ANT+ device type.
pub(super) const BLE_CADENCE: u8 = 1;

#[derive(Default)]
pub(super) struct Timing {
    first_frame: Option<u64>,
    home: Option<u64>,
    companion: Option<u64>,
    storage: Option<u64>,
    /// The recorder state the journal scan ended in (`error` if it failed).
    storage_state: Option<&'static str>,
    gnss_fix: Option<u64>,
    sensors: heapless::Vec<(u8, u64), SENSORS>,
    sensors_dropped: u32,
    dropped_kinds: heapless::Vec<u8, SENSORS>,
    ride_press: Option<u64>,
    recording: Option<u64>,
    /// A press is waiting for the next frame drawn after it.
    press_pending: Option<u64>,
    press_frame_last: Option<u64>,
    press_frame_max: Option<u64>,
}

fn mark(slot: &mut Option<u64>, now: u64, name: &str) {
    if slot.is_none() {
        *slot = Some(now);
        log::info!(target: "sdk.timing", "{}_ms={}", name, now);
    }
}

impl Timing {
    pub(super) fn first_frame(&mut self, now: u64) {
        mark(&mut self.first_frame, now, "first_frame");
    }
    pub(super) fn home(&mut self, now: u64) {
        mark(&mut self.home, now, "home");
    }
    pub(super) fn companion(&mut self, now: u64) {
        mark(&mut self.companion, now, "companion");
    }
    /// The journal scan ended in `state`; only a usable journal is a milestone.
    pub(super) fn storage(&mut self, now: u64, state: &'static str, usable: bool) {
        if self.storage_state.is_none() {
            self.storage_state = Some(state);
            if usable {
                mark(&mut self.storage, now, "storage");
            } else {
                log::warn!(target: "sdk.timing", "storage_failed_ms={} state={}", now, state);
            }
        }
    }
    pub(super) fn gnss_fix(&mut self, now: u64) {
        mark(&mut self.gnss_fix, now, "gnss_fix");
    }
    pub(super) fn sensor(&mut self, kind: u8, now: u64) {
        if self.sensors.iter().any(|(seen, _)| *seen == kind) {
            return;
        }
        if self.sensors.push((kind, now)).is_ok() {
            log::info!(target: "sdk.timing", "sensor_ms={} type={}", now, kind);
        } else if !self.dropped_kinds.contains(&kind) {
            // Counted once per kind, so a busy kind does not inflate it.
            let _ = self.dropped_kinds.push(kind);
            self.sensors_dropped = self.sensors_dropped.saturating_add(1);
        }
    }
    /// The rider pressed start; recording is confirmed once the recorder says
    /// so. Press-to-frame figures restart with the new ride; the start press
    /// itself still waits for its frame.
    pub(super) fn ride_press(&mut self, now: u64) {
        self.ride_press = Some(now);
        self.recording = None;
        self.press_frame_last = None;
        self.press_frame_max = None;
        log::info!(target: "sdk.timing", "ride_press_ms={}", now);
    }
    pub(super) fn recording(&mut self, now: u64) {
        if self.ride_press.is_some() && self.recording.is_none() {
            self.recording = Some(now);
            log::info!(target: "sdk.timing", "recording_ms={}", now);
        }
    }
    /// A button press reached VANA during a ride.
    pub(super) fn press(&mut self, now: u64) {
        self.press_pending.get_or_insert(now);
    }
    /// A frame was drawn; a pending press is answered by it.
    pub(super) fn frame(&mut self, now: u64) {
        if let Some(pressed) = self.press_pending.take() {
            let latency = now.saturating_sub(pressed);
            self.press_frame_last = Some(latency);
            self.press_frame_max = Some(self.press_frame_max.unwrap_or(0).max(latency));
        }
    }

    pub(super) fn report(&self, output: &mut impl Write) {
        fn field(output: &mut impl Write, name: &str, value: Option<u64>) {
            let _ = match value {
                Some(value) => write!(output, "{name}={value} "),
                None => write!(output, "{name}=- "),
            };
        }
        field(output, "first_frame_ms", self.first_frame);
        field(output, "home_ms", self.home);
        field(output, "companion_ms", self.companion);
        field(output, "storage_ms", self.storage);
        let _ = write!(
            output,
            "storage_state={} ",
            self.storage_state.unwrap_or("-")
        );
        field(output, "gnss_fix_ms", self.gnss_fix);
        let _ = output.write_str("sensors=");
        if self.sensors.is_empty() {
            let _ = output.write_str("-");
        }
        for (index, (kind, at)) in self.sensors.iter().enumerate() {
            let _ = write!(
                output,
                "{}{}:{}",
                if index > 0 { "," } else { "" },
                kind,
                at
            );
        }
        let _ = write!(output, " sensors_dropped={} ", self.sensors_dropped);
        field(output, "ride_press_ms", self.ride_press);
        field(output, "recording_ms", self.recording);
        field(output, "press_frame_last_ms", self.press_frame_last);
        field(output, "press_frame_max_ms", self.press_frame_max);
    }
}
