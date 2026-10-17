//! ANT sensors the rider deliberately selected: one peer per device type, in
//! stable tile order, and their persisted field.
//!
//! The field is `[count]` followed by `count` identities of
//! `[device_type][device_number lo][device_number hi][transmission_type]`, in
//! tile order. VANA's preference record versions it; link states are never
//! stored.
use device_api::ant::{CHANNEL_CAPACITY, Identity};

const IDENTITY_BYTES: usize = 4;
pub const FIELD_CAPACITY: usize = 1 + CHANNEL_CAPACITY * IDENTITY_BYTES;

/// The field does not describe saved sensors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unreadable;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Saved([Option<Identity>; CHANNEL_CAPACITY]);

/// Zero is a wildcard on the wire; a saved peer must name one sensor.
const fn explicit(peer: Identity) -> bool {
    peer.device_type != 0 && peer.device_number != 0 && peer.transmission_type != 0
}

impl Saved {
    pub fn get(&self, slot: usize) -> Option<Identity> {
        self.0.get(slot).copied().flatten()
    }

    pub fn peers(&self) -> impl Iterator<Item = (usize, Identity)> + '_ {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(slot, peer)| Some((slot, (*peer)?)))
    }

    pub fn of_type(&self, kind: u8) -> Option<(usize, Identity)> {
        self.peers().find(|(_, peer)| peer.device_type == kind)
    }

    pub fn contains(&self, peer: Identity) -> bool {
        self.peers().any(|(_, saved)| saved == peer)
    }

    pub fn is_full(&self) -> bool {
        self.0.iter().all(Option::is_some)
    }

    /// Select a peer, replacing its type in place or taking the first free
    /// slot. Returns the slot, or `None` when every slot holds another type.
    pub fn select(&mut self, peer: Identity) -> Option<usize> {
        if !explicit(peer) {
            return None;
        }
        let slot = self
            .of_type(peer.device_type)
            .map(|(slot, _)| slot)
            .or_else(|| self.0.iter().position(Option::is_none))?;
        self.0[slot] = Some(peer);
        Some(slot)
    }

    /// Remove the peer of a type; other tiles keep their positions.
    pub fn remove(&mut self, kind: u8) -> Option<Identity> {
        let (slot, peer) = self.of_type(kind)?;
        self.0[slot] = None;
        Some(peer)
    }

    pub fn encode(&self) -> heapless::Vec<u8, FIELD_CAPACITY> {
        let mut output = heapless::Vec::new();
        let peers = self.peers().count() as u8;
        // Cannot fail: the capacity covers every slot.
        let _ = output.push(peers);
        for (_, peer) in self.peers() {
            let [lo, hi] = peer.device_number.to_le_bytes();
            let _ = output.extend_from_slice(&[peer.device_type, lo, hi, peer.transmission_type]);
        }
        output
    }

    pub fn decode(payload: &[u8]) -> Result<Self, Unreadable> {
        let [count, identities @ ..] = payload else {
            return Err(Unreadable);
        };
        let count = usize::from(*count);
        if count > CHANNEL_CAPACITY || identities.len() != count * IDENTITY_BYTES {
            return Err(Unreadable);
        }
        let mut saved = Self::default();
        for (slot, bytes) in identities
            .as_chunks::<IDENTITY_BYTES>()
            .0
            .iter()
            .enumerate()
        {
            let peer = Identity {
                device_type: bytes[0],
                device_number: u16::from_le_bytes([bytes[1], bytes[2]]),
                transmission_type: bytes[3],
            };
            if !explicit(peer) || saved.of_type(peer.device_type).is_some() {
                return Err(Unreadable);
            }
            saved.0[slot] = Some(peer);
        }
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(device_type: u8, device_number: u16) -> Identity {
        Identity {
            device_type,
            device_number,
            transmission_type: 1,
        }
    }

    #[test]
    fn one_peer_per_type_with_stable_slots_and_explicit_replacement() {
        let mut saved = Saved::default();
        assert_eq!(saved.select(peer(120, 1)), Some(0));
        assert_eq!(saved.select(peer(11, 2)), Some(1));
        assert_eq!(saved.select(peer(121, 3)), Some(2));
        // Same-type replacement keeps the tile position.
        assert_eq!(saved.select(peer(11, 9)), Some(1));
        assert_eq!(saved.of_type(11), Some((1, peer(11, 9))));
        assert!(!saved.contains(peer(11, 2)));
        assert_eq!(saved.remove(120), Some(peer(120, 1)));
        assert_eq!(saved.remove(120), None);
        assert_eq!(saved.get(1), Some(peer(11, 9)));
        assert_eq!(saved.select(peer(40, 4)), Some(0));
        assert_eq!(
            saved.select(Identity {
                device_number: 0,
                ..peer(17, 1)
            }),
            None
        );
        for kind in 1..=7 {
            assert!(saved.select(peer(kind, 5)).is_some());
        }
        assert!(saved.is_full());
        assert_eq!(saved.select(peer(8, 5)), None);
        assert_eq!(saved.select(peer(1, 6)), Some(3));
    }

    #[test]
    fn record_round_trips_full_identity_in_tile_order() {
        let mut saved = Saved::default();
        saved.select(peer(120, 1));
        saved.select(Identity {
            device_type: 40,
            device_number: 0xbeef,
            transmission_type: 0xa5,
        });
        saved.select(peer(11, 7));
        saved.remove(120);
        let record = saved.encode();
        assert_eq!(record.as_slice(), [2, 40, 0xef, 0xbe, 0xa5, 11, 7, 0, 1]);
        let restored = Saved::decode(&record).unwrap();
        // Tiles keep their order; gaps close on restart.
        assert_eq!(restored.get(0), saved.get(1));
        assert_eq!(restored.get(1), saved.get(2));
        assert_eq!(restored.encode(), record);
        assert_eq!(Saved::decode(&[0]), Ok(Saved::default()));
        let mut full = Saved::default();
        for kind in 1..=10 {
            full.select(peer(kind, u16::from(kind)));
        }
        assert_eq!(Saved::decode(&full.encode()), Ok(full));
    }

    #[test]
    fn inconsistent_fields_are_unreadable() {
        for payload in [
            &[][..],
            &[0, 0],
            &[1],
            &[1, 120, 1, 0],
            &[1, 120, 1, 0, 1, 0],
            &[1, 120, 0, 0, 1],
            &[2, 120, 1, 0, 1, 120, 2, 0, 1],
            &[11],
        ] {
            assert_eq!(Saved::decode(payload), Err(Unreadable), "{payload:?}");
        }
    }
}
