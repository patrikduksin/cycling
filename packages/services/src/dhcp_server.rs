//! A DHCPv4 server for a local-only network: addresses from a small pool,
//! a subnet mask and the server identifier, and deliberately no router or
//! DNS options. A client then treats the network as local and keeps its
//! default route (a phone's cellular data) elsewhere.
//!
//! Replies are broadcast to port 68, so clients without an address receive
//! them. Leases are keyed by hardware address and kept in a fixed table.

/// Addresses handed out: `.2` up to `.2 + LEASES - 1` of the server's /24.
pub const LEASES: usize = 8;
pub const LEASE_SECS: u32 = 2 * 60 * 60;
/// Room a reply needs.
pub const REPLY_SIZE: usize = 300;

const MAGIC: [u8; 4] = [99, 130, 83, 99];
const OPTIONS: usize = 240;
const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const DECLINE: u8 = 4;
const ACK: u8 = 5;
const NAK: u8 = 6;
const RELEASE: u8 = 7;
const INFORM: u8 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Lease {
    mac: [u8; 6],
    /// Acknowledged, not only offered.
    bound: bool,
    expires_ms: u64,
}

pub struct Server {
    address: [u8; 4],
    leases: [Option<Lease>; LEASES],
}

/// What a client message asked for.
struct Message<'a> {
    kind: u8,
    xid: [u8; 4],
    flags: [u8; 2],
    ciaddr: [u8; 4],
    mac: [u8; 6],
    requested: Option<[u8; 4]>,
    server: Option<[u8; 4]>,
    _bytes: &'a [u8],
}

fn parse(bytes: &[u8]) -> Option<Message<'_>> {
    if bytes.len() < OPTIONS + 4 || bytes[0] != 1 || bytes[1] != 1 || bytes[2] != 6 {
        return None;
    }
    if bytes[236..240] != MAGIC {
        return None;
    }
    let mut kind = None;
    let mut requested = None;
    let mut server = None;
    let mut at = OPTIONS;
    while at < bytes.len() {
        let code = bytes[at];
        if code == 255 {
            break;
        }
        if code == 0 {
            at += 1;
            continue;
        }
        let length = usize::from(*bytes.get(at + 1)?);
        let value = bytes.get(at + 2..at + 2 + length)?;
        match (code, length) {
            (53, 1) => kind = Some(value[0]),
            (50, 4) => requested = Some([value[0], value[1], value[2], value[3]]),
            (54, 4) => server = Some([value[0], value[1], value[2], value[3]]),
            _ => {}
        }
        at += 2 + length;
    }
    let mut mac = [0; 6];
    mac.copy_from_slice(&bytes[28..34]);
    Some(Message {
        kind: kind?,
        xid: [bytes[4], bytes[5], bytes[6], bytes[7]],
        flags: [bytes[10], bytes[11]],
        ciaddr: [bytes[12], bytes[13], bytes[14], bytes[15]],
        mac,
        requested,
        server,
        _bytes: bytes,
    })
}

impl Server {
    /// Serve the /24 around `address`, which must end in `.1`.
    pub fn new(address: [u8; 4]) -> Self {
        Self {
            address,
            leases: [None; LEASES],
        }
    }

    fn address_of(&self, index: usize) -> [u8; 4] {
        [
            self.address[0],
            self.address[1],
            self.address[2],
            2 + index as u8,
        ]
    }

    fn index_of(&self, address: [u8; 4]) -> Option<usize> {
        let index = usize::from(address[3]).checked_sub(2)?;
        (address[..3] == self.address[..3] && index < LEASES).then_some(index)
    }

    /// Stations holding an acknowledged, unexpired lease.
    pub fn bound(&self, now_ms: u64) -> usize {
        self.leases
            .iter()
            .flatten()
            .filter(|lease| lease.bound && lease.expires_ms > now_ms)
            .count()
    }

    /// The lease index for `mac`: its own, else a free or expired one.
    fn allocate(&mut self, mac: [u8; 6], now_ms: u64) -> Option<usize> {
        if let Some(index) = self
            .leases
            .iter()
            .position(|l| l.is_some_and(|l| l.mac == mac))
        {
            return Some(index);
        }
        let free = self
            .leases
            .iter()
            .position(|l| l.is_none_or(|l| l.expires_ms <= now_ms))?;
        self.leases[free] = Some(Lease {
            mac,
            bound: false,
            expires_ms: now_ms + 60_000,
        });
        Some(free)
    }

    /// Answer one client message; returns the reply length in `out`, to be
    /// broadcast to port 68.
    pub fn handle(
        &mut self,
        request: &[u8],
        now_ms: u64,
        out: &mut [u8; REPLY_SIZE],
    ) -> Option<usize> {
        let message = parse(request)?;
        match message.kind {
            DISCOVER => {
                let index = self.allocate(message.mac, now_ms)?;
                Some(self.reply(&message, OFFER, Some(index), out))
            }
            REQUEST => {
                if message.server.is_some_and(|server| server != self.address) {
                    // The client chose another server: forget our offer.
                    self.forget(message.mac);
                    return None;
                }
                let wanted = message
                    .requested
                    .or((message.ciaddr != [0; 4]).then_some(message.ciaddr));
                let own = self
                    .leases
                    .iter()
                    .position(|l| l.is_some_and(|l| l.mac == message.mac));
                let index = match (wanted.and_then(|a| self.index_of(a)), own) {
                    (Some(wanted), Some(own)) if wanted == own => Some(own),
                    (Some(wanted), None)
                        if self.leases[wanted].is_none_or(|l| l.expires_ms <= now_ms) =>
                    {
                        Some(wanted)
                    }
                    (None, Some(own)) if wanted.is_none() => Some(own),
                    _ => None,
                };
                let Some(index) = index else {
                    return Some(self.reply(&message, NAK, None, out));
                };
                self.leases[index] = Some(Lease {
                    mac: message.mac,
                    bound: true,
                    expires_ms: now_ms + u64::from(LEASE_SECS) * 1000,
                });
                Some(self.reply(&message, ACK, Some(index), out))
            }
            DECLINE | RELEASE => {
                self.forget(message.mac);
                None
            }
            INFORM => Some(self.reply(&message, ACK, None, out)),
            _ => None,
        }
    }

    fn forget(&mut self, mac: [u8; 6]) {
        for lease in &mut self.leases {
            if lease.is_some_and(|l| l.mac == mac) {
                *lease = None;
            }
        }
    }

    fn reply(
        &self,
        message: &Message,
        kind: u8,
        index: Option<usize>,
        out: &mut [u8; REPLY_SIZE],
    ) -> usize {
        out.fill(0);
        out[0] = 2;
        out[1] = 1;
        out[2] = 6;
        out[4..8].copy_from_slice(&message.xid);
        // Broadcast flag, since the client may not have an address yet.
        out[10..12].copy_from_slice(&[message.flags[0] | 0x80, message.flags[1]]);
        if kind == ACK && index.is_none() {
            out[12..16].copy_from_slice(&message.ciaddr);
        }
        if let Some(index) = index {
            out[16..20].copy_from_slice(&self.address_of(index));
        }
        out[20..24].copy_from_slice(&self.address);
        out[28..34].copy_from_slice(&message.mac);
        out[236..240].copy_from_slice(&MAGIC);
        let mut at = OPTIONS;
        let mut option = |code: u8, value: &[u8]| {
            out[at] = code;
            out[at + 1] = value.len() as u8;
            out[at + 2..at + 2 + value.len()].copy_from_slice(value);
            at += 2 + value.len();
        };
        option(53, &[kind]);
        option(54, &self.address);
        if kind != NAK {
            option(1, &[255, 255, 255, 0]);
            if index.is_some() {
                option(51, &LEASE_SECS.to_be_bytes());
            }
        }
        out[at] = 255;
        at + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn client(kind: u8, mac: u8, options: &[(u8, &[u8])]) -> Vec<u8> {
        let mut bytes = std::vec![0u8; 240];
        bytes[0] = 1;
        bytes[1] = 1;
        bytes[2] = 6;
        bytes[4..8].copy_from_slice(&[1, 2, 3, mac]);
        bytes[28..34].copy_from_slice(&[2, 0, 0, 0, 0, mac]);
        bytes[236..240].copy_from_slice(&MAGIC);
        bytes.extend_from_slice(&[53, 1, kind]);
        for (code, value) in options {
            bytes.push(*code);
            bytes.push(value.len() as u8);
            bytes.extend_from_slice(value);
        }
        bytes.push(255);
        bytes
    }

    fn options(reply: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut found = Vec::new();
        let mut at = OPTIONS;
        while reply[at] != 255 {
            let length = usize::from(reply[at + 1]);
            found.push((reply[at], reply[at + 2..at + 2 + length].to_vec()));
            at += 2 + length;
        }
        found
    }

    const SERVER: [u8; 4] = [192, 168, 4, 1];

    #[test]
    fn offers_and_acknowledges_without_router_or_dns() {
        let mut server = Server::new(SERVER);
        let mut out = [0; REPLY_SIZE];
        let n = server
            .handle(&client(DISCOVER, 7, &[]), 0, &mut out)
            .unwrap();
        assert_eq!(out[0], 2);
        assert_eq!(&out[16..20], &[192, 168, 4, 2]);
        assert_eq!(out[10] & 0x80, 0x80);
        let offered = options(&out[..n]);
        let codes: Vec<u8> = offered.iter().map(|(c, _)| *c).collect();
        assert_eq!(codes, [53, 54, 1, 51]);
        assert!(
            !codes.contains(&3) && !codes.contains(&6),
            "no router, no DNS"
        );
        assert_eq!(server.bound(0), 0);
        let request = client(REQUEST, 7, &[(50, &[192, 168, 4, 2]), (54, &SERVER)]);
        let n = server.handle(&request, 10, &mut out).unwrap();
        assert_eq!(options(&out[..n])[0], (53, std::vec![ACK]));
        assert_eq!(&out[16..20], &[192, 168, 4, 2]);
        assert_eq!(server.bound(10), 1);
        // A second phone gets the next address; a renewal keeps the first.
        server
            .handle(&client(DISCOVER, 8, &[]), 20, &mut out)
            .unwrap();
        assert_eq!(&out[16..20], &[192, 168, 4, 3]);
        server
            .handle(
                &client(REQUEST, 7, &[(50, &[192, 168, 4, 2])]),
                30,
                &mut out,
            )
            .unwrap();
        assert_eq!(&out[16..20], &[192, 168, 4, 2]);
        assert_eq!(
            server.bound(u64::from(LEASE_SECS) * 1000 + 31),
            0,
            "leases expire"
        );
    }

    #[test]
    fn refuses_foreign_or_taken_addresses_and_frees_on_release() {
        let mut server = Server::new(SERVER);
        let mut out = [0; REPLY_SIZE];
        server
            .handle(&client(DISCOVER, 1, &[]), 0, &mut out)
            .unwrap();
        server
            .handle(&client(REQUEST, 1, &[(50, &[192, 168, 4, 2])]), 0, &mut out)
            .unwrap();
        let n = server
            .handle(&client(REQUEST, 2, &[(50, &[192, 168, 4, 2])]), 0, &mut out)
            .unwrap();
        assert_eq!(options(&out[..n])[0], (53, std::vec![NAK]));
        let n = server
            .handle(&client(REQUEST, 2, &[(50, &[10, 0, 0, 5])]), 0, &mut out)
            .unwrap();
        assert_eq!(options(&out[..n])[0], (53, std::vec![NAK]));
        assert!(
            server
                .handle(&client(REQUEST, 3, &[(54, &[192, 168, 4, 9])]), 0, &mut out)
                .is_none()
        );
        assert!(
            server
                .handle(&client(RELEASE, 1, &[]), 0, &mut out)
                .is_none()
        );
        assert_eq!(server.bound(0), 0);
        assert!(server.handle(&[0; 10], 0, &mut out).is_none());
    }

    #[test]
    fn a_full_pool_offers_nothing() {
        let mut server = Server::new(SERVER);
        let mut out = [0; REPLY_SIZE];
        for mac in 0..LEASES as u8 {
            assert!(
                server
                    .handle(&client(DISCOVER, mac, &[]), 0, &mut out)
                    .is_some()
            );
        }
        assert!(
            server
                .handle(&client(DISCOVER, 99, &[]), 0, &mut out)
                .is_none()
        );
        // Unacknowledged offers lapse after a minute.
        assert!(
            server
                .handle(&client(DISCOVER, 99, &[]), 60_001, &mut out)
                .is_some()
        );
    }
}
