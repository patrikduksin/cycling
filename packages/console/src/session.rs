//! Bounded command and log session over a nonblocking byte transport.
use crate::log_record::Line;
use crate::protocol::Lines;
use crate::protocol::Request;
use serde::Serialize;

const BYTES: usize = 1536;

#[derive(Serialize)]
struct Reply<'a> {
    r#type: &'static str,
    id: u32,
    status: &'static str,
    ms: u64,
    data: &'a str,
}
pub struct Terminal<T> {
    usb: T,
    lines: Lines,
    current: [u8; BYTES],
    length: usize,
    offset: usize,
    reply: [u8; BYTES],
    reply_len: usize,
    rejected: u32,
    sent: u32,
    tx_max_us: u64,
    reboot: Option<(u64, bool)>,
}
impl<T: device_api::console::Console> Terminal<T> {
    pub fn new(usb: T) -> Self {
        Self {
            usb,
            lines: Lines::default(),
            current: [0; BYTES],
            length: 0,
            offset: 0,
            reply: [0; BYTES],
            reply_len: 0,
            rejected: 0,
            sent: 0,
            tx_max_us: 0,
            reboot: None,
        }
    }
    pub fn reply(&mut self, id: u32, status: &'static str, data: &str, now: u64) {
        let reply = Reply {
            r#type: "reply",
            id,
            status,
            ms: now,
            data,
        };
        match serde_json_core::to_slice(&reply, &mut self.reply[..BYTES - 1]) {
            Ok(n) => {
                self.reply[n] = b'\n';
                self.reply_len = n + 1;
            }
            Err(_) => {
                self.rejected = self.rejected.saturating_add(1);
                let fallback = Reply {
                    r#type: "reply",
                    id,
                    status: "ENCODING_FAILED",
                    ms: now,
                    data: "",
                };
                let n = serde_json_core::to_slice(&fallback, &mut self.reply[..BYTES - 1]).unwrap();
                self.reply[n] = b'\n';
                self.reply_len = n + 1;
            }
        }
    }
    /// Whole records cannot interleave. Replies win between records; each poll
    /// offers at most 64 bytes and never waits on a disconnected host.
    pub fn pump(
        &mut self,
        mut next_log: impl FnMut() -> Option<Line>,
        mut clock_us: impl FnMut() -> u64,
    ) {
        let started = clock_us();
        if self.offset == self.length {
            if self.reply_len > 0 {
                self.length = self.reply_len;
                self.current[..self.length].copy_from_slice(&self.reply[..self.length]);
                self.reply_len = 0;
            } else if let Some(record) = next_log() {
                self.length = record.as_bytes().len();
                self.current[..self.length].copy_from_slice(record.as_bytes());
            } else {
                return;
            }
            self.offset = 0;
        }
        for _ in 0..64 {
            if self.offset == self.length {
                break;
            }
            if !self.usb.write(self.current[self.offset]) {
                break;
            }
            self.offset += 1;
        }
        self.usb.flush();
        self.tx_max_us = self.tx_max_us.max(clock_us().saturating_sub(started));
    }
    /// Return the requested restart mode after draining the reply, or after the
    /// one-second deadline if the host cannot receive it. The caller owns reset.
    pub fn reboot_due(&self, now: u64) -> Option<bool> {
        let (at, panic) = self.reboot?;
        if now >= at + 1000
            || (now >= at + 100 && self.reply_len == 0 && self.offset == self.length)
        {
            Some(panic)
        } else {
            None
        }
    }
    pub fn schedule_reboot(&mut self, now: u64, panic: bool) {
        self.reboot = Some((now, panic));
    }
    pub fn rejected(&self) -> u32 {
        self.rejected
    }
    pub fn sent(&self) -> u32 {
        self.sent
    }
    pub fn tx_max_us(&self) -> u64 {
        self.tx_max_us
    }
    pub fn command_completed(&mut self) {
        self.sent = self.sent.saturating_add(1);
    }
    pub fn request(&mut self, now: u64) -> Option<Request> {
        if self.reply_len != 0 {
            return None;
        }
        for _ in 0..64 {
            let Some(byte) = self.usb.read() else {
                break;
            };
            if let Some(result) = self.lines.push(byte) {
                return match result {
                    Ok(request) => Some(request),
                    Err(error) => {
                        self.rejected = self.rejected.saturating_add(1);
                        self.reply(
                            0,
                            "INVALID",
                            match error {
                                crate::protocol::Error::Invalid => "malformed command",
                                crate::protocol::Error::Overlong => "line exceeds 256 bytes",
                            },
                            now,
                        );
                        None
                    }
                };
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::collections::VecDeque;
    use std::vec::Vec;

    #[derive(Default)]
    struct Transport {
        input: VecDeque<u8>,
        output: Vec<u8>,
        blocked: bool,
    }
    impl device_api::console::Console for Transport {
        fn read(&mut self) -> Option<u8> {
            self.input.pop_front()
        }
        fn write(&mut self, byte: u8) -> bool {
            if self.blocked {
                return false;
            }
            self.output.push(byte);
            true
        }
        fn flush(&mut self) {}
    }
    fn line(bytes: &[u8]) -> Line {
        let mut line = Line {
            bytes: [0; crate::log_record::RECORD_BYTES],
            len: bytes.len(),
        };
        line.bytes[..bytes.len()].copy_from_slice(bytes);
        line
    }

    #[test]
    fn records_stay_whole_and_replies_take_priority_between_records() {
        let mut session = Terminal::new(Transport::default());
        let first = [b'x'; 100];
        let mut logs = VecDeque::from([line(&first), line(b"next log\n")]);
        session.pump(|| logs.pop_front(), || 0);
        assert_eq!(session.usb.output.len(), 64);
        session.reply(7, "OK", "done", 20);
        session.pump(|| logs.pop_front(), || 0);
        assert_eq!(&session.usb.output, &first);
        session.pump(|| logs.pop_front(), || 0);
        assert_eq!(logs.len(), 1);
        for _ in 0..4 {
            session.pump(|| logs.pop_front(), || 0);
        }
        let output = core::str::from_utf8(&session.usb.output[100..]).unwrap();
        assert_eq!(
            output,
            "{\"type\":\"reply\",\"id\":7,\"status\":\"OK\",\"ms\":20,\"data\":\"done\"}\nnext log\n"
        );
    }

    #[test]
    fn reboot_waits_for_reply_but_disconnected_host_cannot_prevent_it() {
        let mut session = Terminal::new(Transport::default());
        session.reply(1, "ACCEPTED", "", 0);
        session.schedule_reboot(0, true);
        session.usb.blocked = true;
        session.pump(|| None, || 0);
        assert_eq!(session.reboot_due(100), None);
        assert_eq!(session.reboot_due(999), None);
        assert_eq!(session.reboot_due(1000), Some(true));
        session.usb.blocked = false;
        for _ in 0..3 {
            session.pump(|| None, || 0);
        }
        assert_eq!(session.reboot_due(99), None);
        assert_eq!(session.reboot_due(100), Some(true));
    }

    #[test]
    fn malformed_request_gets_a_reply_before_reading_the_next_request() {
        let mut transport = Transport::default();
        transport.input.extend(b"bad\nCMD 4 HELP\n");
        let mut session = Terminal::new(transport);
        assert!(session.request(0).is_none());
        assert_eq!(session.rejected(), 1);
        assert!(session.request(0).is_none());
        session.pump(|| None, || 0);
        let request = session.request(0).unwrap();
        assert_eq!(request.id, 4);
        assert_eq!(request.command, crate::protocol::Command::Help);
    }
}
