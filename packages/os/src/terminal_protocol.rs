//! Bounded ordinary terminal framing, independent of USB and the test harness.

pub const MAX_LINE: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    #[cfg(feature = "cycling")]
    Domain {
        bytes: [u8; 128],
        length: usize,
    },
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
    Ble,
    BleReconnect,
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
        #[cfg(feature = "cycling")]
        "RIDE" | "EXPORT" => {
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
            Some("RECONNECT") => Command::WifiReconnect,
            _ => return Err(Error::Invalid),
        },
        "BLE" => match words.next() {
            None => Command::Ble,
            Some("RECONNECT") => Command::BleReconnect,
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
