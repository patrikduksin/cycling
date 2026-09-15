//! Bounded ordinary terminal framing, independent of USB and the test harness.

/// A transport accepting a request has queued work, not completed a reconnect.
pub fn request_status(result: Result<(), device_api::observation::Error>) -> &'static str {
    use device_api::observation::Error;
    match result {
        Ok(()) => "ACCEPTED",
        Err(Error::Unsupported) => "UNSUPPORTED",
        Err(Error::Unavailable) => "UNAVAILABLE",
        Err(Error::Failed) => "FAILED",
        Err(Error::Invalid) => "INVALID",
    }
}

pub const MAX_LINE: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Power(Option<device_api::power::Operation>),
    PowerShutdownAfter(u32),
    Peripheral(crate::commands::peripherals::Command),
    Harness(firmware_shell::harness::Command),
    #[cfg(feature = "cycling")]
    Domain {
        bytes: [u8; 128],
        length: usize,
    },
    Ant,
    AntScan(u8),
    AntStop,
    AntConnect(device_api::ant::Identity),
    AntDisconnect(u8),
    AntChannel(u8),
    AntDevices,
    AntRead,
    Help,
    Info,
    Status,
    Position,
    Input,
    Battery,
    Time,
    Settings,
    SetBrightness(u8),
    SetTimezone(i16),
    SetIdle(u16, u8),
    Save,
    Activity,
    Wifi,
    WifiReconnect,
    WifiScan,
    WifiNetworks,
    WifiConfigure(device_api::connectivity::WifiConfig),
    WifiDisconnect,
    WifiForget,
    Ble,
    BleReconnect,
    BleScan,
    BlePeers,
    BleSelect(device_api::ble_transport::Selection, u8),
    BleDisconnect,
    BleForget,
    BleEcho,
    Storage,
    Display(u16),
    Restart,
    Test(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub id: u32,
    pub command: Command,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Invalid,
    Overlong,
}

pub struct Lines {
    bytes: [u8; MAX_LINE],
    length: usize,
    discard: bool,
}

impl Default for Lines {
    fn default() -> Self {
        Self {
            bytes: [0; MAX_LINE],
            length: 0,
            discard: false,
        }
    }
}

impl Lines {
    /// Consume through newline after an invalid/overlong record. Exactly one
    /// error is emitted, and suffixes cannot become unrelated commands.
    pub fn push(&mut self, byte: u8) -> Option<Result<Request, Error>> {
        if byte == b'\n' {
            let result = if self.discard {
                Err(Error::Overlong)
            } else {
                parse(&self.bytes[..self.length])
            };
            self.length = 0;
            self.discard = false;
            return Some(result);
        }
        if !self.discard {
            if self.length == MAX_LINE {
                self.discard = true;
            } else {
                self.bytes[self.length] = byte;
                self.length += 1;
            }
        }
        None
    }
}

pub fn parse(bytes: &[u8]) -> Result<Request, Error> {
    if bytes.len() > MAX_LINE {
        return Err(Error::Overlong);
    }
    let line = core::str::from_utf8(bytes).map_err(|_| Error::Invalid)?;
    let mut words = line.split_ascii_whitespace();
    if words.next() != Some("CMD") {
        return Err(Error::Invalid);
    }
    let id = words
        .next()
        .ok_or(Error::Invalid)?
        .parse()
        .map_err(|_| Error::Invalid)?;
    let verb = words.next().ok_or(Error::Invalid)?;
    let command = match verb {
        "HARNESS" => {
            Command::Harness(firmware_shell::harness::parse(&mut words).ok_or(Error::Invalid)?)
        }
        #[cfg(feature = "cycling")]
        "RIDE" | "EXPORT" | "RADAR" => {
            let text = &line[line.find(verb).ok_or(Error::Invalid)?..];
            let mut bytes = [0; 128];
            if text.len() > bytes.len() {
                return Err(Error::Overlong);
            }
            bytes[..text.len()].copy_from_slice(text.as_bytes());
            for _ in words.by_ref() {}
            Command::Domain {
                bytes,
                length: text.len(),
            }
        }
        "ANT" => match words.next() {
            None => Command::Ant,
            Some("SCAN") => {
                let seconds: u8 = words
                    .next()
                    .ok_or(Error::Invalid)?
                    .parse()
                    .map_err(|_| Error::Invalid)?;
                if !(1..=60).contains(&seconds) {
                    return Err(Error::Invalid);
                }
                Command::AntScan(seconds)
            }
            Some("STOP") => Command::AntStop,
            Some("DEVICES") => Command::AntDevices,
            Some("READ") => Command::AntRead,
            Some("DISCONNECT") => Command::AntDisconnect(
                words
                    .next()
                    .ok_or(Error::Invalid)?
                    .parse()
                    .map_err(|_| Error::Invalid)?,
            ),
            Some("CHANNEL") => Command::AntChannel(
                words
                    .next()
                    .ok_or(Error::Invalid)?
                    .parse()
                    .map_err(|_| Error::Invalid)?,
            ),
            Some("CONNECT") => Command::AntConnect(device_api::ant::Identity {
                device_type: words
                    .next()
                    .ok_or(Error::Invalid)?
                    .parse()
                    .map_err(|_| Error::Invalid)?,
                device_number: words
                    .next()
                    .ok_or(Error::Invalid)?
                    .parse()
                    .map_err(|_| Error::Invalid)?,
                transmission_type: words
                    .next()
                    .ok_or(Error::Invalid)?
                    .parse()
                    .map_err(|_| Error::Invalid)?,
            }),
            _ => return Err(Error::Invalid),
        },
        name @ ("MMC" | "SOUND" | "GNSS" | "PRESSURE" | "MOTION" | "COMPANION") => {
            Command::Peripheral(
                crate::commands::peripherals::parse(name, &mut words).ok_or(Error::Invalid)?,
            )
        }
        "POWER" => match words.next() {
            None | Some("STATUS") => Command::Power(None),
            Some("SHUTDOWN") => match words.next() {
                None => Command::Power(Some(device_api::power::Operation::Shutdown)),
                Some("AFTER") => {
                    let delay_ms = words
                        .next()
                        .ok_or(Error::Invalid)?
                        .parse::<u32>()
                        .map_err(|_| Error::Invalid)?;
                    if delay_ms > device_api::power::MAX_DELAY_MS {
                        return Err(Error::Invalid);
                    }
                    Command::PowerShutdownAfter(delay_ms)
                }
                _ => return Err(Error::Invalid),
            },
            Some("SLEEP") => Command::Power(Some(device_api::power::Operation::Sleep)),
            Some("WAKE") => Command::Power(Some(device_api::power::Operation::Wake)),
            _ => return Err(Error::Invalid),
        },
        "HELP" => Command::Help,
        "INFO" => Command::Info,
        "STATUS" => Command::Status,
        "POSITION" => Command::Position,
        "INPUT" => Command::Input,
        "BATTERY" => Command::Battery,
        "TIME" => Command::Time,
        "SETTINGS" => Command::Settings,
        "SAVE" => Command::Save,
        "ACTIVITY" => Command::Activity,
        "STORAGE" => Command::Storage,
        "RESTART" => Command::Restart,
        "WIFI" => match words.next() {
            None => Command::Wifi,
            Some("CONNECT" | "RECONNECT") => Command::WifiReconnect,
            Some("SCAN") => Command::WifiScan,
            Some("NETWORKS") => Command::WifiNetworks,
            Some("DISCONNECT") => Command::WifiDisconnect,
            Some("FORGET") => Command::WifiForget,
            Some("CONFIG") => {
                let auth = match words.next() {
                    Some("WPA2") => false,
                    Some("WPA3") => true,
                    _ => return Err(Error::Invalid),
                };
                let ssid = hex::<32>(words.next().ok_or(Error::Invalid)?)?;
                let password = hex::<63>(words.next().ok_or(Error::Invalid)?)?;
                Command::WifiConfigure(
                    device_api::connectivity::WifiConfig::new(ssid.bytes(), password.bytes(), auth)
                        .ok_or(Error::Invalid)?,
                )
            }
            _ => return Err(Error::Invalid),
        },
        "BLE" => match words.next() {
            None => Command::Ble,
            Some("CONNECT" | "RECONNECT") => Command::BleReconnect,
            Some("SCAN") => Command::BleScan,
            Some("PEERS") => Command::BlePeers,
            Some("DISCONNECT") => Command::BleDisconnect,
            Some("FORGET") => Command::BleForget,
            Some("ECHO") => Command::BleEcho,
            Some("SELECT") => {
                let profile = match words.next() {
                    Some("HRS") => 1,
                    Some("CSC") => 2,
                    _ => return Err(Error::Invalid),
                };
                let name = hex::<32>(words.next().ok_or(Error::Invalid)?)?;
                let address = match words.next().ok_or(Error::Invalid)? {
                    "-" => None,
                    value => {
                        let mut bytes = [0; 6];
                        decode_hex(value, &mut bytes)?;
                        if value.len() != 12 {
                            return Err(Error::Invalid);
                        }
                        Some(bytes)
                    }
                };
                if name.is_empty() && address.is_none() {
                    return Err(Error::Invalid);
                }
                Command::BleSelect(
                    device_api::ble_transport::Selection {
                        name,
                        address,
                        service: 0,
                        characteristic: 0,
                    },
                    profile,
                )
            }
            _ => return Err(Error::Invalid),
        },
        "BRIGHTNESS" => {
            let value: u8 = words
                .next()
                .ok_or(Error::Invalid)?
                .parse()
                .map_err(|_| Error::Invalid)?;
            if value > 100 {
                return Err(Error::Invalid);
            }
            Command::SetBrightness(value)
        }
        "TIMEZONE" => {
            let value: i16 = words
                .next()
                .ok_or(Error::Invalid)?
                .parse()
                .map_err(|_| Error::Invalid)?;
            if !(-720..=840).contains(&value) {
                return Err(Error::Invalid);
            }
            Command::SetTimezone(value)
        }
        "IDLE" => {
            let seconds = words
                .next()
                .ok_or(Error::Invalid)?
                .parse()
                .map_err(|_| Error::Invalid)?;
            let level = words
                .next()
                .ok_or(Error::Invalid)?
                .parse()
                .map_err(|_| Error::Invalid)?;
            if level > 100 {
                return Err(Error::Invalid);
            }
            Command::SetIdle(seconds, level)
        }
        "DISPLAY" => Command::Display(
            u16::from_str_radix(words.next().ok_or(Error::Invalid)?, 16)
                .map_err(|_| Error::Invalid)?,
        ),
        "TEST" => Command::Test(
            words
                .next()
                .ok_or(Error::Invalid)?
                .parse()
                .map_err(|_| Error::Invalid)?,
        ),
        _ => return Err(Error::Invalid),
    };
    if words.next().is_some() {
        return Err(Error::Invalid);
    }
    Ok(Request { id, command })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ant_commands_are_bounded_and_explicit() {
        assert_eq!(
            parse(b"CMD 1 ANT SCAN 60").unwrap().command,
            Command::AntScan(60)
        );
        assert!(matches!(
            parse(b"CMD 2 ANT CONNECT 40 1234 5").unwrap().command,
            Command::AntConnect(_)
        ));
        for input in [
            b"CMD 1 ANT SCAN 0".as_slice(),
            b"CMD 1 ANT SCAN 61",
            b"CMD 1 ANT CONNECT 40 1234",
            b"CMD 1 ANT STOP extra",
            b"CMD 1 ANT CONNECT 256 1234 5",
        ] {
            assert_eq!(parse(input), Err(Error::Invalid));
        }
    }
    #[test]
    fn malformed_and_trailing_arguments_cannot_act() {
        for line in [
            "CMD 1 BRIGHTNESS 101",
            "CMD 1 SAVE extra",
            "CMD -1 SAVE",
            "CMD 1 WIFI RECONNECT extra",
            "CMD 1 TIMEZONE 841",
            "CMD 1 DISPLAY 10000",
            "SAVE",
            "CMD 1 BLE nope",
        ] {
            assert_eq!(parse(line.as_bytes()), Err(Error::Invalid), "{line}");
        }
        assert_eq!(
            parse(b"CMD 42 WIFI RECONNECT"),
            Ok(Request {
                id: 42,
                command: Command::WifiReconnect
            })
        );
    }
    #[test]
    fn overlong_input_discards_suffix_and_recovers() {
        let mut lines = Lines::default();
        for _ in 0..MAX_LINE + 1 {
            assert_eq!(lines.push(b'x'), None);
        }
        for byte in b"CMD 1 RESTART" {
            assert_eq!(lines.push(*byte), None);
        }
        assert_eq!(lines.push(b'\n'), Some(Err(Error::Overlong)));
        for byte in b"CMD 2 STATUS\r" {
            assert_eq!(lines.push(*byte), None);
        }
        assert_eq!(
            lines.push(b'\n'),
            Some(Ok(Request {
                id: 2,
                command: Command::Status
            }))
        );
    }
}

#[cfg(test)]
mod request_status_tests {
    use super::request_status;
    use device_api::observation::Availability;
    use device_api::observation::Error;
    #[test]
    fn reconnect_rejections_do_not_queue_or_claim_acceptance() {
        for (availability, expected) in [
            (Availability::Unsupported, "UNSUPPORTED"),
            (Availability::Initializing, "UNAVAILABLE"),
            (Availability::Unconfigured, "UNAVAILABLE"),
            (Availability::Failed, "FAILED"),
            (Availability::Ready, "ACCEPTED"),
        ] {
            let mut queued = false;
            let result = availability.require_ready().map(|()| queued = true);
            assert_eq!(request_status(result), expected);
            assert_eq!(queued, availability == Availability::Ready);
        }
        assert_eq!(request_status(Err(Error::Invalid)), "INVALID");
        assert_eq!(request_status(Err(Error::Failed)), "FAILED");
    }
}

fn decode_hex(value: &str, output: &mut [u8]) -> Result<usize, Error> {
    if !value.len().is_multiple_of(2) || value.len() / 2 > output.len() {
        return Err(Error::Invalid);
    }
    for (slot, pair) in output
        .iter_mut()
        .zip(value.as_bytes().as_chunks::<2>().0.iter())
    {
        let digit = |v: u8| match v {
            b'0'..=b'9' => Some(v - b'0'),
            b'a'..=b'f' => Some(v - b'a' + 10),
            b'A'..=b'F' => Some(v - b'A' + 10),
            _ => None,
        };
        *slot =
            digit(pair[0]).ok_or(Error::Invalid)? * 16 + digit(pair[1]).ok_or(Error::Invalid)?;
    }
    Ok(value.len() / 2)
}
fn hex<const N: usize>(value: &str) -> Result<device_api::connectivity::Text<N>, Error> {
    if value == "-" {
        return Ok(device_api::connectivity::Text::empty());
    }
    let mut bytes = [0; N];
    let n = decode_hex(value, &mut bytes)?;
    device_api::connectivity::Text::new(&bytes[..n]).ok_or(Error::Invalid)
}

#[cfg(test)]
mod connectivity_tests {
    use super::*;
    #[test]
    fn runtime_credentials_use_bounded_hex_and_cannot_inject_commands() {
        let command = parse(b"CMD 1 WIFI CONFIG WPA2 54657374 70617373776f7264")
            .unwrap()
            .command;
        let Command::WifiConfigure(config) = command else {
            panic!("configuration expected")
        };
        assert_eq!(config.ssid.text(), "Test");
        assert_eq!(config.password.text(), "password");
        for value in [
            b"CMD 1 WIFI CONFIG WPA2 ff 70617373776f7264".as_slice(),
            b"CMD 1 WIFI CONFIG WPA2 54657374 00",
            b"CMD 1 WIFI CONFIG WPA2 54657374 70617373776f7264 RESTART",
            b"CMD 1 BLE SELECT HRS - -",
            b"CMD 1 BLE SELECT HRS 00 -",
            b"CMD 1 BLE SELECT NEW 61 -",
            b"CMD 1 BLE SELECT HRS - abcdef",
        ] {
            assert!(parse(value).is_err());
        }
        for (command, expected_profile, expected_name, expected_address) in [
            (
                b"CMD 1 BLE SELECT CSC 54657374 060504030201".as_slice(),
                2,
                b"Test".as_slice(),
                Some([6, 5, 4, 3, 2, 1]),
            ),
            (b"CMD 1 BLE SELECT HRS 54657374 -", 1, b"Test", None),
            (
                b"CMD 1 BLE SELECT HRS - 060504030201",
                1,
                b"",
                Some([6, 5, 4, 3, 2, 1]),
            ),
        ] {
            let Command::BleSelect(peer, profile) = parse(command).unwrap().command else {
                panic!("selection expected");
            };
            assert_eq!(profile, expected_profile);
            assert_eq!(peer.name.bytes(), expected_name);
            assert_eq!(peer.address, expected_address);
            // Application handlers resolve the requested profile's UUIDs.
            assert_eq!((peer.service, peer.characteristic), (0, 0));
        }
    }
}
