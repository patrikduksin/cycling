use device_api::ant::{Discovery, Identity};

/// How many sensors the rider can save, whatever their types. Not the
/// radio's channel count: several saved sensors share one type's channel.
pub const SAVED_CAPACITY: usize = 10;
const ENTRY_BYTES: usize = 5;
pub const FIELD_CAPACITY: usize = 1 + SAVED_CAPACITY * ENTRY_BYTES;

/// The field does not describe saved sensors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unreadable;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Saved([Option<(Identity, u8)>; SAVED_CAPACITY]);

/// Zero is a wildcard on the wire; a saved peer must name one sensor.
const fn explicit(peer: Identity) -> bool {
    peer.device_type != 0 && peer.device_number != 0 && peer.transmission_type != 0
}

impl Saved {
    pub fn get(&self, slot: usize) -> Option<Identity> {
        self.0.get(slot).copied().flatten().map(|(peer, _)| peer)
    }

    pub fn peers(&self) -> impl Iterator<Item = (usize, Identity)> + '_ {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(slot, entry)| Some((slot, (*entry)?.0)))
    }

    pub fn slot_of(&self, peer: Identity) -> Option<usize> {
        self.peers()
            .find(|(_, saved)| *saved == peer)
            .map(|(slot, _)| slot)
    }

    pub fn contains(&self, peer: Identity) -> bool {
        self.slot_of(peer).is_some()
    }

    pub fn is_full(&self) -> bool {
        self.0.iter().all(Option::is_some)
    }

    pub fn count_of(&self, kind: u8) -> usize {
        self.peers()
            .filter(|(_, peer)| peer.device_type == kind)
            .count()
    }

    /// The sensor of this type the radio connects: the one used last.
    pub fn active(&self, kind: u8) -> Option<(usize, Identity)> {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(slot, entry)| Some((slot, (*entry)?)))
            .filter(|(_, (peer, _))| peer.device_type == kind)
            .min_by_key(|(_, (_, rank))| *rank)
            .map(|(slot, (peer, _))| (slot, peer))
    }

    pub fn is_active(&self, slot: usize) -> bool {
        self.get(slot)
            .is_some_and(|peer| self.active(peer.device_type).map(|(s, _)| s) == Some(slot))
    }

    /// Save a sensor in the first free slot as the one used last, so it is
    /// its type's active sensor. One already saved keeps its slot and is
    /// used. Returns the slot, or `None` when every slot is taken or the
    /// identity has a wildcard zero.
    pub fn add(&mut self, peer: Identity) -> Option<usize> {
        if !explicit(peer) {
            return None;
        }
        if let Some(slot) = self.slot_of(peer) {
            self.used(peer);
            return Some(slot);
        }
        let slot = self.0.iter().position(Option::is_none)?;
        for (_, rank) in self.0.iter_mut().flatten() {
            *rank += 1;
        }
        self.0[slot] = Some((peer, 0));
        Some(slot)
    }

    /// Forget one sensor; other tiles keep their positions and order of use.
    pub fn remove(&mut self, peer: Identity) -> Option<usize> {
        let slot = self.slot_of(peer)?;
        let (_, removed) = self.0[slot].take()?;
        for (_, rank) in self.0.iter_mut().flatten() {
            if *rank > removed {
                *rank -= 1;
            }
        }
        Some(slot)
    }

    /// Make a saved sensor its type's active one. Returns whether that
    /// changed which sensor of its type is active; nothing else moves then,
    /// so an unchanged choice never rewrites the record.
    pub fn used(&mut self, peer: Identity) -> bool {
        if self.active(peer.device_type).map(|(_, active)| active) == Some(peer) {
            return false;
        }
        let Some(slot) = self.slot_of(peer) else {
            return false;
        };
        let Some((_, from)) = self.0[slot] else {
            return false;
        };
        for (_, rank) in self.0.iter_mut().flatten() {
            if *rank < from {
                *rank += 1;
            }
        }
        self.0[slot] = Some((peer, 0));
        true
    }

    /// The sensor of `kind` the radio should connect, from what a discovery
    /// heard: the active one while it is heard, else the strongest heard,
    /// else the active one still. `None` when nothing of the type is saved.
    pub fn choose(&self, kind: u8, heard: &[Option<Discovery>]) -> Option<Identity> {
        let (_, active) = self.active(kind)?;
        let saved = heard
            .iter()
            .flatten()
            .filter(|found| found.identity.device_type == kind && self.contains(found.identity));
        if saved.clone().any(|found| found.identity == active) {
            return Some(active);
        }
        let strongest = saved.max_by_key(|found| found.rssi);
        Some(strongest.map_or(active, |found| found.identity))
    }

    pub fn encode(&self) -> heapless::Vec<u8, FIELD_CAPACITY> {
        let mut output = heapless::Vec::new();
        let entries = self.0.iter().flatten();
        // Cannot fail: the capacity covers every slot.
        let _ = output.push(entries.clone().count() as u8);
        for (peer, rank) in entries {
            let [lo, hi] = peer.device_number.to_le_bytes();
            let _ = output.extend_from_slice(&[
                peer.device_type,
                lo,
                hi,
                peer.transmission_type,
                *rank,
            ]);
        }
        output
    }

    /// The field at the start of `bytes`, and the bytes after it.
    pub fn decode(bytes: &[u8]) -> Result<(Self, &[u8]), Unreadable> {
        let [count, rest @ ..] = bytes else {
            return Err(Unreadable);
        };
        let count = usize::from(*count);
        if count > SAVED_CAPACITY {
            return Err(Unreadable);
        }
        let (entries, rest) = rest
            .split_at_checked(count * ENTRY_BYTES)
            .ok_or(Unreadable)?;
        let mut saved = Self::default();
        let mut ranks = [false; SAVED_CAPACITY];
        for (slot, bytes) in entries.as_chunks::<ENTRY_BYTES>().0.iter().enumerate() {
            let peer = Identity {
                device_type: bytes[0],
                device_number: u16::from_le_bytes([bytes[1], bytes[2]]),
                transmission_type: bytes[3],
            };
            let rank = bytes[4];
            let fresh = ranks.get_mut(usize::from(rank)).filter(|seen| !**seen);
            let Some(seen) = fresh.filter(|_| usize::from(rank) < count) else {
                return Err(Unreadable);
            };
            if !explicit(peer) || saved.contains(peer) {
                return Err(Unreadable);
            }
            *seen = true;
            saved.0[slot] = Some((peer, rank));
        }
        Ok((saved, rest))
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

    fn heard(found: &[(Identity, i8)]) -> [Option<Discovery>; 8] {
        let mut heard = [None; 8];
        for (slot, (identity, rssi)) in found.iter().enumerate() {
            heard[slot] = Some(Discovery {
                identity: *identity,
                rssi: *rssi,
                seen_ms: 0,
            });
        }
        heard
    }

    #[test]
    fn several_sensors_of_a_type_and_the_one_added_last_is_active() {
        let (bike, spare) = (peer(11, 1), peer(11, 2));
        let mut saved = Saved::default();
        assert_eq!(saved.add(peer(120, 7)), Some(0));
        assert_eq!(saved.add(bike), Some(1));
        assert_eq!(saved.add(spare), Some(2));
        assert_eq!(saved.count_of(11), 2);
        assert_eq!(saved.active(11), Some((2, spare)));
        assert!(saved.is_active(2) && !saved.is_active(1));
        assert!(saved.used(bike));
        assert!(!saved.used(bike), "already active: nothing moves");
        assert_eq!(saved.active(11), Some((1, bike)));
        assert_eq!(saved.active(120), Some((0, peer(120, 7))));
        assert_eq!(saved.remove(bike), Some(1));
        assert_eq!(saved.remove(bike), None);
        assert_eq!(saved.active(11), Some((2, spare)));
        assert_eq!(saved.get(1), None);
        assert_eq!(saved.add(peer(40, 4)), Some(1));
        assert_eq!(saved.add(spare), Some(2), "re-adding keeps its slot");
        assert_eq!(
            saved.add(Identity {
                device_number: 0,
                ..peer(17, 1)
            }),
            None
        );
    }

    #[test]
    fn ten_saved_whatever_their_types() {
        let mut saved = Saved::default();
        for number in 1..=10 {
            assert!(saved.add(peer(11, number)).is_some());
        }
        assert!(saved.is_full());
        assert_eq!(saved.add(peer(11, 11)), None);
        assert_eq!(saved.add(peer(120, 1)), None);
        assert_eq!(saved.active(11), Some((9, peer(11, 10))));
    }

    #[test]
    fn the_choice_keeps_a_heard_active_sensor_else_takes_the_strongest_heard() {
        let (a, b, c) = (peer(11, 1), peer(11, 2), peer(11, 3));
        let mut saved = Saved::default();
        for sensor in [a, b, c] {
            saved.add(sensor);
        }
        saved.used(a);
        let stranger = peer(11, 9);
        assert_eq!(saved.choose(11, &heard(&[])), Some(a), "nothing heard");
        assert_eq!(saved.choose(11, &heard(&[(stranger, -40)])), Some(a));
        assert_eq!(saved.choose(11, &heard(&[(b, -80), (c, -60)])), Some(c));
        assert_eq!(saved.choose(11, &heard(&[(c, -40), (a, -90)])), Some(a));
        assert_eq!(saved.choose(120, &heard(&[(b, -60)])), None);
    }

    #[test]
    fn record_round_trips_identities_and_order_of_use_in_tile_order() {
        let mut saved = Saved::default();
        saved.add(peer(120, 1));
        saved.add(Identity {
            device_type: 11,
            device_number: 0xbeef,
            transmission_type: 0xa5,
        });
        saved.add(peer(11, 7));
        saved.remove(peer(120, 1));
        let record = saved.encode();
        assert_eq!(
            record.as_slice(),
            [2, 11, 0xef, 0xbe, 0xa5, 1, 11, 7, 0, 1, 0]
        );
        let (restored, rest) = Saved::decode(&record).unwrap();
        assert!(rest.is_empty());
        // Tiles keep their order; gaps close on restart.
        assert_eq!(restored.get(0), saved.get(1));
        assert_eq!(restored.get(1), saved.get(2));
        assert_eq!(restored.active(11), saved.active(11).map(|(_, p)| (1, p)));
        assert_eq!(restored.encode(), record);
        assert_eq!(Saved::decode(&[0, 7]), Ok((Saved::default(), &[7][..])));
        let mut full = Saved::default();
        for number in 1..=10 {
            full.add(peer(11, number));
        }
        assert_eq!(Saved::decode(&full.encode()), Ok((full, &[][..])));
    }

    #[test]
    fn inconsistent_fields_are_unreadable() {
        for payload in [
            &[][..],
            &[1],
            &[1, 120, 1, 0, 1],
            &[1, 120, 0, 0, 1, 0],
            &[1, 120, 1, 0, 1, 1],
            &[2, 120, 1, 0, 1, 0, 120, 1, 0, 1, 1],
            &[2, 120, 1, 0, 1, 0, 11, 2, 0, 1, 0],
            &[11],
        ] {
            assert_eq!(Saved::decode(payload), Err(Unreadable), "{payload:?}");
        }
    }
}
