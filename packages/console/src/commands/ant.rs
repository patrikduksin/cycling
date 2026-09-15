//! ANT terminal formatting. Diagnostic reads have an independent bounded queue.
use crate::protocol::Command;
use core::fmt::Write;
use device_api::ant::{Admission, Error};

/// Preserve the original control-command wire statuses.
fn admission_status(result: Result<Admission, Error>) -> &'static str {
    match result {
        Ok(_) => "ACCEPTED",
        Err(Error::Busy) => "BUSY",
        Err(Error::Unavailable) => "UNAVAILABLE",
        Err(_) => "STATE",
    }
}
fn send_admission_status(result: Result<Admission, Error>) -> &'static str {
    match result {
        Ok(_) => "ACCEPTED",
        Err(Error::Busy) => "BUSY",
        Err(Error::UnsupportedType) => "UNSUPPORTED",
        Err(Error::Capacity) => "CAPACITY",
        Err(Error::Unavailable) => "UNAVAILABLE",
        Err(Error::Disconnected) => "DISCONNECTED",
        Err(Error::StaleGeneration) => "STALE",
        Err(Error::Uncertain) => "UNCERTAIN",
        Err(Error::InvalidIdentity | Error::InvalidDuration) => "INVALID",
    }
}

pub fn execute(
    command: Command,
    ant: &mut impl device_api::ant::Ant,
    now: u64,
    output: &mut impl Write,
) -> Option<&'static str> {
    let mut status = "OK";
    match command {
        Command::Ant => {
            let _ = write!(output, "scanning={} ", ant.scanning());
            for channel in ant.channels(now).iter().flatten() {
                if let Some(peer) = channel.selected {
                    let _ = write!(
                        output,
                        "type={} link={} packets={} dropped={} stale={}; ",
                        peer.device_type,
                        channel.link.name(),
                        channel.packets,
                        channel.dropped_packets,
                        channel.stale
                    );
                }
            }
        }
        Command::AntChannel(kind) => {
            if let Some(channel) = ant.channel(kind, now) {
                let _ = write!(output, "{:?}", channel);
            } else {
                status = "UNAVAILABLE";
            }
        }
        Command::AntDevices => {
            for device in ant.discoveries().iter().flatten() {
                let p = device.identity;
                let _ = write!(
                    output,
                    "type={} number={} transmission={} rssi={} age_ms={}; ",
                    p.device_type,
                    p.device_number,
                    p.transmission_type,
                    device.rssi,
                    now.saturating_sub(device.seen_ms)
                );
            }
        }
        Command::AntRead => {
            if let Some(packet) = ant.take_diagnostic_packet() {
                let _ = write!(output, "{:?}", packet);
            } else {
                status = "EMPTY";
            }
        }
        Command::AntScan(seconds) => {
            status = admission_status(ant.request(
                device_api::ant::AntOperation::Scan(u32::from(seconds) * 1000),
                now,
            ));
        }
        Command::AntStop => {
            status = admission_status(ant.request(device_api::ant::AntOperation::StopScan, now));
        }
        Command::AntConnect(peer) => {
            status =
                admission_status(ant.request(device_api::ant::AntOperation::Connect(peer), now));
        }
        Command::AntDisconnect(kind) => {
            status =
                admission_status(ant.request(device_api::ant::AntOperation::Disconnect(kind), now));
        }
        Command::AntCapabilities => {
            let _ = write!(
                output,
                "{:?} availability={:?} scan={:?}",
                ant.capabilities(),
                ant.availability(),
                ant.scan()
            );
        }
        Command::AntOperation(id) => {
            if let Some(observation) = ant.send_status(device_api::ant::OperationId(id)) {
                let _ = write!(output, "{:?}", observation);
            } else {
                status = "UNAVAILABLE";
            }
        }
        Command::AntSend {
            identity,
            generation,
            data,
        } => {
            let result = ant.request(
                device_api::ant::AntOperation::Send {
                    target: device_api::ant::Target {
                        identity,
                        generation,
                    },
                    data,
                },
                now,
            );
            if let Ok(device_api::ant::Admission::SendQueued(id)) = result {
                let _ = write!(output, "operation={} stage=queued delivery=unknown", id.0);
            }
            status = send_admission_status(result);
        }
        _ => return None,
    }
    Some(status)
}
