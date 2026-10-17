//! The upload format of one workout, version 1. Hosts write it (`mise run
//! workout-load` from JSON); the device checks and stores it unchanged.
//! Multi-byte fields are little-endian.
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..2 | `VW` |
//! | 2 | Version, 1 |
//! | 3 | Test: 0 none, 1 twenty minutes, 2 ramp |
//! | 4 | Name length n, at most 20, then n name bytes |
//! | then 1 | Step count s, 1..=12 |
//! | then 1 | Block count b, 0..=8 |
//! | then s × (15 + m) | Step: intensity, length kind, length u32, target kind, three u16, a zero byte, note length m, m note bytes |
//! | then b × 3 | Block: first step, steps, runs |
//! | last 4 | CRC32 of everything before it |
//!
//! Intensity: 0 warm up, 1 work, 2 recovery, 3 cool down, 4 test. Length
//! kind: 0 seconds, 1 metres, 2 open. Target kind and its values: 0 free; 1
//! % FTP low, high; 2 watts low, high; 3 heart zone; 4 % LTHR low, high; 5
//! cadence low, high; 6 ramp start watts, step watts, step seconds. Unused
//! values are zero. A decoded workout must also pass `Workout::validate`.
use super::model::{
    Block, Intensity, Length, MAX_BLOCKS, MAX_STEPS, NAME_BYTES, Name, Step, Target, Test, Workout,
};
use firmware_services::checksum::crc32;
use firmware_services::le::{get_u16, get_u32, put_u16, put_u32};

const MAGIC: &[u8; 2] = b"VW";
pub const VERSION: u8 = 1;
const STEP_FIXED: usize = 15;
/// The largest encoded workout.
pub const ENCODED_MAX: usize =
    5 + NAME_BYTES + 2 + MAX_STEPS * (STEP_FIXED + NAME_BYTES) + MAX_BLOCKS * 3 + 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Magic,
    Version,
    Length,
    Checksum,
    Field,
    Invalid(super::model::Invalid),
}

pub fn encode(workout: &Workout) -> heapless::Vec<u8, ENCODED_MAX> {
    let mut out = heapless::Vec::new();
    // Cannot fail: ENCODED_MAX covers the largest workout `Workout` holds.
    let mut push = |bytes: &[u8]| {
        let _ = out.extend_from_slice(bytes);
    };
    let test = match workout.test {
        None => 0,
        Some(Test::Twenty) => 1,
        Some(Test::Ramp) => 2,
    };
    push(MAGIC);
    push(&[VERSION, test, workout.name.as_bytes().len() as u8]);
    push(workout.name.as_bytes());
    push(&[workout.steps().len() as u8, workout.blocks().len() as u8]);
    for step in workout.steps() {
        let mut fixed = [0u8; STEP_FIXED];
        fixed[0] = Intensity::ALL
            .iter()
            .position(|i| *i == step.intensity)
            .unwrap_or(0) as u8;
        let (kind, value) = match step.length {
            Length::Time(secs) => (0, u32::from(secs)),
            Length::Distance(metres) => (1, metres),
            Length::Open => (2, 0),
        };
        fixed[1] = kind;
        put_u32(&mut fixed, 2, value);
        let (kind, values) = match step.target {
            Target::Free => (0, [0, 0, 0]),
            Target::Ftp { low, high } => (1, [low.into(), high.into(), 0]),
            Target::Watts { low, high } => (2, [low, high, 0]),
            Target::HeartZone(zone) => (3, [zone.into(), 0, 0]),
            Target::Lthr { low, high } => (4, [low.into(), high.into(), 0]),
            Target::Cadence { low, high } => (5, [low.into(), high.into(), 0]),
            Target::Ramp { start, step, every } => (6, [start, step, every]),
        };
        fixed[6] = kind;
        for (i, value) in values.into_iter().enumerate() {
            put_u16(&mut fixed, 7 + 2 * i, value);
        }
        fixed[13] = 0;
        fixed[14] = step.note.as_bytes().len() as u8;
        push(&fixed);
        push(step.note.as_bytes());
    }
    for block in workout.blocks() {
        push(&[block.first, block.len, block.count]);
    }
    let crc = crc32(&out);
    let mut tail = [0; 4];
    put_u32(&mut tail, 0, crc);
    let _ = out.extend_from_slice(&tail);
    out
}

pub fn decode(bytes: &[u8]) -> Result<Workout, DecodeError> {
    if bytes.len() < 4 || bytes.len() > ENCODED_MAX {
        return Err(DecodeError::Length);
    }
    let (body, tail) = bytes.split_at(bytes.len() - 4);
    if !body.starts_with(MAGIC) {
        return Err(DecodeError::Magic);
    }
    if body.get(2) != Some(&VERSION) {
        return Err(DecodeError::Version);
    }
    if crc32(body) != get_u32(tail, 0) {
        return Err(DecodeError::Checksum);
    }
    let mut reader = Reader { bytes: body, at: 3 };
    let test = match reader.byte()? {
        0 => None,
        1 => Some(Test::Twenty),
        2 => Some(Test::Ramp),
        _ => return Err(DecodeError::Field),
    };
    let name = reader.name()?;
    let step_count = usize::from(reader.byte()?);
    let block_count = usize::from(reader.byte()?);
    if step_count > MAX_STEPS || block_count > MAX_BLOCKS {
        return Err(DecodeError::Field);
    }
    let mut steps = [Step::new(Intensity::Work, Length::Open, Target::Free); MAX_STEPS];
    for step in steps.iter_mut().take(step_count) {
        let fixed = reader.take(STEP_FIXED)?;
        let intensity = *Intensity::ALL
            .get(usize::from(fixed[0]))
            .ok_or(DecodeError::Field)?;
        let value = get_u32(fixed, 2);
        let length = match fixed[1] {
            0 => Length::Time(u16::try_from(value).map_err(|_| DecodeError::Field)?),
            1 => Length::Distance(value),
            2 if value == 0 => Length::Open,
            _ => return Err(DecodeError::Field),
        };
        let [a, b, c] = [7, 9, 11].map(|at| get_u16(fixed, at));
        let small = |v: u16| u8::try_from(v).map_err(|_| DecodeError::Field);
        let target = match fixed[6] {
            0 => Target::Free,
            1 => Target::Ftp {
                low: small(a)?,
                high: small(b)?,
            },
            2 => Target::Watts { low: a, high: b },
            3 => Target::HeartZone(small(a)?),
            4 => Target::Lthr {
                low: small(a)?,
                high: small(b)?,
            },
            5 => Target::Cadence {
                low: small(a)?,
                high: small(b)?,
            },
            6 => Target::Ramp {
                start: a,
                step: b,
                every: c,
            },
            _ => return Err(DecodeError::Field),
        };
        if fixed[13] != 0 || (fixed[6] != 6 && c != 0) {
            return Err(DecodeError::Field);
        }
        reader.at -= 1;
        let note = reader.name()?;
        *step = Step {
            intensity,
            length,
            target,
            note,
        };
    }
    let mut blocks = [Block {
        first: 0,
        len: 0,
        count: 0,
    }; MAX_BLOCKS];
    for block in blocks.iter_mut().take(block_count) {
        let bytes = reader.take(3)?;
        *block = Block {
            first: bytes[0],
            len: bytes[1],
            count: bytes[2],
        };
    }
    if reader.at != body.len() {
        return Err(DecodeError::Length);
    }
    let mut workout = Workout::new("", &steps[..step_count], &blocks[..block_count], test);
    workout.name = name;
    workout.validate().map_err(DecodeError::Invalid)?;
    Ok(workout)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let bytes = self
            .bytes
            .get(self.at..self.at + len)
            .ok_or(DecodeError::Length)?;
        self.at += len;
        Ok(bytes)
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    /// A length-prefixed name, upper case printable ASCII only.
    fn name(&mut self) -> Result<Name, DecodeError> {
        let len = usize::from(self.byte()?);
        if len > NAME_BYTES {
            return Err(DecodeError::Field);
        }
        let text = self.take(len)?;
        let name = Name::from_bytes(text);
        if name.as_bytes() != text {
            return Err(DecodeError::Field);
        }
        Ok(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workout::library::BUILT_IN;

    #[test]
    fn built_ins_round_trip() {
        for workout in BUILT_IN {
            let bytes = encode(workout);
            assert_eq!(decode(&bytes).as_ref(), Ok(workout), "{:?}", workout.name);
        }
    }

    /// The layout is what host loaders write; pin a small workout's bytes.
    #[test]
    fn layout_is_pinned() {
        let workout = Workout::new(
            "AB",
            &[Step::new(
                Intensity::Work,
                Length::Time(300),
                Target::Ftp { low: 90, high: 95 },
            )
            .note("GO")],
            &[Block {
                first: 0,
                len: 1,
                count: 2,
            }],
            None,
        );
        let bytes = encode(&workout);
        let body = [
            b'V', b'W', 1, 0, 2, b'A', b'B', 1, 1, // header
            1, 0, 0x2c, 0x01, 0, 0, 1, 90, 0, 95, 0, 0, 0, 0, 2, b'G', b'O', // step
            0, 1, 2, // block
        ];
        assert_eq!(&bytes[..body.len()], &body);
        assert_eq!(bytes.len(), body.len() + 4);
        assert_eq!(get_u32(&bytes, body.len()), crc32(&body));
    }

    #[test]
    fn damaged_and_foreign_bytes_are_refused() {
        let bytes = encode(BUILT_IN[1]);
        assert_eq!(decode(&bytes[..3]), Err(DecodeError::Length));
        let mut wrong = bytes.clone();
        wrong[0] = b'X';
        assert_eq!(decode(&wrong), Err(DecodeError::Magic));
        let mut newer = bytes.clone();
        newer[2] = 2;
        assert_eq!(decode(&newer), Err(DecodeError::Version));
        let mut flipped = bytes.clone();
        flipped[10] ^= 0x40;
        assert_eq!(decode(&flipped), Err(DecodeError::Checksum));
        // A well-formed record of an invalid workout: a zero-length step.
        let zero = Workout::new(
            "Z",
            &[Step::new(Intensity::Work, Length::Time(0), Target::Free)],
            &[],
            None,
        );
        assert!(matches!(
            decode(&encode(&zero)),
            Err(DecodeError::Invalid(_))
        ));
        // Lower case names are not what hosts write.
        let mut lower = heapless::Vec::<u8, 64>::new();
        let _ = lower.extend_from_slice(&[b'V', b'W', 1, 0, 1, b'a', 1, 0]);
        let _ = lower.extend_from_slice(&[1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let crc = crc32(&lower);
        let _ = lower.extend_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&lower), Err(DecodeError::Field));
    }
}
