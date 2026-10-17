//! Bounded command and log session over a nonblocking byte transport.
use crate::log_record::Line;
use crate::protocol::Lines;
use crate::protocol::Request;
use serde::Serialize;

const BYTES: usize = 1536;
/// Bytes offered per poll, one less than a full-speed bulk packet. Each flush
/// then ends in a short packet, which completes the host's read transfer. A
/// record ending on a full 64-byte packet without a zero-length packet stays
/// queued on the host (Linux cdc-acm) until unrelated later output arrives.
const PACKET: usize = 63;

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
    transfer: bool,
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
            transfer: false,
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
    /// offers at most one short packet and never waits on a disconnected host.
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
            } else if self.transfer {
                // No log record may start between the reply and the session.
                return;
            } else if let Some(record) = next_log() {
                self.length = record.as_bytes().len();
                self.current[..self.length].copy_from_slice(record.as_bytes());
            } else {
                return;
            }
            self.offset = 0;
        }
        for _ in 0..PACKET {
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
    /// Hand the transport to the binary transfer session once the current
    /// reply has been sent (`transfer_due`).
    pub fn schedule_transfer(&mut self) {
        self.transfer = true;
    }
    /// The transfer was scheduled and every queued reply byte was offered.
    pub fn transfer_due(&self) -> bool {
        self.transfer && self.reply_len == 0 && self.offset == self.length
    }
    /// The transport, for the transfer session.
    pub fn transport(&mut self) -> &mut T {
        &mut self.usb
    }
    /// Back to line commands; bytes of the binary session never become one.
    pub fn end_transfer(&mut self) {
        self.transfer = false;
        self.lines = Lines::default();
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
        if self.reply_len != 0 || self.transfer {
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
        assert_eq!(session.usb.output.len(), PACKET);
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
    fn every_flush_ends_in_a_short_packet() {
        // A 128-byte reply would otherwise end on a full packet and stall on the host.
        let mut session = Terminal::new(Transport::default());
        let data = "x".repeat(128 - 55);
        session.reply(1, "OK", &data, 0);
        let mut flushes = Vec::new();
        for _ in 0..4 {
            let before = session.usb.output.len();
            session.pump(|| None, || 0);
            flushes.push(session.usb.output.len() - before);
        }
        assert_eq!(session.usb.output.len(), 128);
        assert_eq!(*session.usb.output.last().unwrap(), b'\n');
        assert!(flushes.iter().all(|&n| n < 64), "{flushes:?}");
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
    fn a_scheduled_transfer_waits_for_its_reply_and_no_log_follows_it() {
        let mut transport = Transport::default();
        transport.input.extend(b"CMD 4 HELP\n");
        let mut session = Terminal::new(transport);
        let mut logs = VecDeque::from([line(b"log\n")]);
        session.reply(3, "ACCEPTED", "", 0);
        session.schedule_transfer();
        assert!(!session.transfer_due());
        for _ in 0..4 {
            session.pump(|| logs.pop_front(), || 0);
        }
        assert!(session.transfer_due());
        assert_eq!(logs.len(), 1);
        assert!(session.usb.output.ends_with(b"}\n"));
        // Bytes waiting are the session's, never a line command.
        assert!(session.request(0).is_none());
        assert_eq!(session.transport().input.len(), 11);
        session.end_transfer();
        assert!(!session.transfer_due());
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
