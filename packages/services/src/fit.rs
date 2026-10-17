//! Streaming writer for Garmin FIT files: header, definition and data records,
//! and the file CRC. No profile knowledge beyond base types: callers choose
//! the messages and fields.
//!
//! The writer keeps only counters and the running CRC, never the file, so a
//! file of any length streams through small caller-owned buffers. The header
//! carries the size of the data that follows it, so a stream that cannot seek
//! back writes the file twice: once into [`Counter`] to learn the size, then
//! for real. Both passes must emit the same records.
//!
//! Every field has a fixed size, and `None` writes the base type's invalid
//! value, so a record's size depends only on its definition.

/// Seconds from the Unix epoch to the FIT epoch, 1989-12-31T00:00:00Z.
pub const FIT_EPOCH_UNIX_S: u64 = 631_065_600;
/// Bytes of the file header this writer emits.
pub const HEADER_SIZE: usize = 14;
/// Bytes of the trailing file CRC.
pub const CRC_SIZE: usize = 2;
/// FIT protocol 1.0: no developer fields, readable by every decoder.
const PROTOCOL_VERSION: u8 = 0x10;
/// The FIT profile version the field numbers follow (21.40).
const PROFILE_VERSION: u16 = 2140;

/// FIT seconds for a Unix time in milliseconds, or `None` before the FIT
/// epoch or beyond its 32-bit range.
pub fn timestamp(unix_ms: u64) -> Option<u32> {
    (unix_ms / 1000)
        .checked_sub(FIT_EPOCH_UNIX_S)
        .and_then(|seconds| u32::try_from(seconds).ok())
        // 0xFFFFFFFF is the invalid value and 0x10000000 and below are
        // relative system times, not dates.
        .filter(|seconds| *seconds > 0x1000_0000 && *seconds != u32::MAX)
}

/// Degrees times 10^7 to FIT semicircles (2^31 per 180 degrees).
pub fn semicircles(degrees_e7: i32) -> i32 {
    let value = i64::from(degrees_e7) * (1i64 << 31) / 1_800_000_000;
    value.clamp(i64::from(i32::MIN) + 1, i64::from(i32::MAX)) as i32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BaseType {
    Enum = 0x00,
    Sint8 = 0x01,
    Uint8 = 0x02,
    Sint16 = 0x83,
    Uint16 = 0x84,
    Sint32 = 0x85,
    Uint32 = 0x86,
    String = 0x07,
    Uint32z = 0x8c,
}

impl BaseType {
    const fn size(self) -> u8 {
        match self {
            Self::Enum | Self::Sint8 | Self::Uint8 | Self::String => 1,
            Self::Sint16 | Self::Uint16 => 2,
            Self::Sint32 | Self::Uint32 | Self::Uint32z => 4,
        }
    }
}

/// One field of a message definition. Strings take `size` bytes; every
/// other field is one value of its base type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub number: u8,
    pub base: BaseType,
    pub size: u8,
}

impl Field {
    pub const fn new(number: u8, base: BaseType) -> Self {
        Self {
            number,
            base,
            size: base.size(),
        }
    }

    pub const fn string(number: u8, size: u8) -> Self {
        Self {
            number,
            base: BaseType::String,
            size,
        }
    }
}

/// A message layout bound to a local message number (0..16).
#[derive(Clone, Copy, Debug)]
pub struct Message {
    pub local: u8,
    pub global: u16,
    pub fields: &'static [Field],
}

impl Message {
    /// Bytes of one data record, including its header byte.
    pub fn record_size(&self) -> usize {
        1 + self
            .fields
            .iter()
            .map(|field| usize::from(field.size))
            .sum::<usize>()
    }

    /// Bytes of the definition record.
    pub fn definition_size(&self) -> usize {
        6 + 3 * self.fields.len()
    }
}

/// One field value, in definition order. `None` is the invalid value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value<'a> {
    Enum(Option<u8>),
    U8(Option<u8>),
    I8(Option<i8>),
    U16(Option<u16>),
    I16(Option<i16>),
    U32(Option<u32>),
    I32(Option<i32>),
    U32z(Option<u32>),
    /// Truncated to the field size, keeping a terminating zero.
    Str(&'a str),
}

impl Value<'_> {
    fn base(&self) -> BaseType {
        match self {
            Self::Enum(_) => BaseType::Enum,
            Self::U8(_) => BaseType::Uint8,
            Self::I8(_) => BaseType::Sint8,
            Self::U16(_) => BaseType::Uint16,
            Self::I16(_) => BaseType::Sint16,
            Self::U32(_) => BaseType::Uint32,
            Self::I32(_) => BaseType::Sint32,
            Self::U32z(_) => BaseType::Uint32z,
            Self::Str(_) => BaseType::String,
        }
    }
}

/// Where encoded bytes go. A sink must accept every byte it is given; callers
/// size their buffers from [`Message::record_size`].
pub trait Sink {
    fn put(&mut self, bytes: &[u8]);
}

/// Counts bytes for the sizing pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counter(pub usize);

impl Sink for Counter {
    fn put(&mut self, bytes: &[u8]) {
        self.0 += bytes.len();
    }
}

/// Garmin's FIT CRC-16.
pub fn crc16(mut crc: u16, bytes: &[u8]) -> u16 {
    const TABLE: [u16; 16] = [
        0x0000, 0xcc01, 0xd801, 0x1400, 0xf001, 0x3c00, 0x2800, 0xe401, 0xa001, 0x6c00, 0x7800,
        0xb401, 0x5000, 0x9c01, 0x8801, 0x4400,
    ];
    for &byte in bytes {
        for nibble in [byte & 0xf, byte >> 4] {
            let tmp = TABLE[usize::from(crc & 0xf)];
            crc = ((crc >> 4) & 0x0fff) ^ tmp ^ TABLE[usize::from(nibble)];
        }
    }
    crc
}

/// Running state of one file being written. The sink is passed to each call,
/// so the writer can be kept between service steps while buffers drain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Writer {
    crc: u16,
    written: usize,
}

impl Writer {
    /// Start a file whose records and definitions after the header take
    /// `data_size` bytes.
    pub fn start(sink: &mut impl Sink, data_size: u32) -> Self {
        let mut header = [0u8; HEADER_SIZE];
        header[0] = HEADER_SIZE as u8;
        header[1] = PROTOCOL_VERSION;
        header[2..4].copy_from_slice(&PROFILE_VERSION.to_le_bytes());
        header[4..8].copy_from_slice(&data_size.to_le_bytes());
        header[8..12].copy_from_slice(b".FIT");
        let header_crc = crc16(0, &header[..12]);
        header[12..14].copy_from_slice(&header_crc.to_le_bytes());
        let mut writer = Self::default();
        writer.emit(sink, &header);
        writer
    }

    /// Bytes written so far, header included.
    pub fn written(&self) -> usize {
        self.written
    }

    fn emit(&mut self, sink: &mut impl Sink, bytes: &[u8]) {
        self.crc = crc16(self.crc, bytes);
        self.written += bytes.len();
        sink.put(bytes);
    }

    pub fn define(&mut self, sink: &mut impl Sink, message: &Message) {
        debug_assert!(message.local < 16 && message.fields.len() < 256);
        let global = message.global.to_le_bytes();
        self.emit(
            sink,
            &[
                0x40 | message.local,
                0,
                0,
                global[0],
                global[1],
                message.fields.len() as u8,
            ],
        );
        for field in message.fields {
            self.emit(sink, &[field.number, field.size, field.base as u8]);
        }
    }

    /// A data record. `values` follow the definition's fields one to one;
    /// a mismatch is a caller bug.
    pub fn record(&mut self, sink: &mut impl Sink, message: &Message, values: &[Value<'_>]) {
        assert_eq!(values.len(), message.fields.len(), "FIT record arity");
        self.emit(sink, &[message.local]);
        for (field, value) in message.fields.iter().zip(values) {
            assert_eq!(field.base, value.base(), "FIT field {} type", field.number);
            let mut bytes = [0u8; 4];
            let encoded: &[u8] = match *value {
                Value::Enum(v) | Value::U8(v) => {
                    bytes[0] = v.unwrap_or(u8::MAX);
                    &bytes[..1]
                }
                Value::I8(v) => {
                    bytes[0] = v.unwrap_or(i8::MAX) as u8;
                    &bytes[..1]
                }
                Value::U16(v) => {
                    bytes[..2].copy_from_slice(&v.unwrap_or(u16::MAX).to_le_bytes());
                    &bytes[..2]
                }
                Value::I16(v) => {
                    bytes[..2].copy_from_slice(&v.unwrap_or(i16::MAX).to_le_bytes());
                    &bytes[..2]
                }
                Value::U32(v) => {
                    bytes.copy_from_slice(&v.unwrap_or(u32::MAX).to_le_bytes());
                    &bytes
                }
                Value::I32(v) => {
                    bytes.copy_from_slice(&v.unwrap_or(i32::MAX).to_le_bytes());
                    &bytes
                }
                Value::U32z(v) => {
                    bytes.copy_from_slice(&v.unwrap_or(0).to_le_bytes());
                    &bytes
                }
                Value::Str(text) => {
                    let size = usize::from(field.size);
                    let text = &text.as_bytes()[..text.len().min(size.saturating_sub(1))];
                    self.emit(sink, text);
                    for _ in text.len()..size {
                        self.emit(sink, &[0]);
                    }
                    continue;
                }
            };
            self.emit(sink, encoded);
        }
    }

    /// Write the file CRC; the file is complete.
    pub fn finish(&mut self, sink: &mut impl Sink) {
        let crc = self.crc.to_le_bytes();
        self.written += CRC_SIZE;
        sink.put(&crc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    struct Bytes(Vec<u8>);
    impl Sink for Bytes {
        fn put(&mut self, bytes: &[u8]) {
            self.0.extend_from_slice(bytes);
        }
    }

    const MESSAGE: Message = Message {
        local: 3,
        global: 20,
        fields: &[
            Field::new(253, BaseType::Uint32),
            Field::new(3, BaseType::Uint8),
            Field::new(0, BaseType::Sint32),
            Field::string(8, 6),
        ],
    };

    #[test]
    fn crc_matches_the_sdk_check_value() {
        // A whole file, CRC included, checks to zero.
        let mut out = Bytes(Vec::new());
        let mut writer = Writer::start(&mut out, 0);
        writer.define(&mut out, &MESSAGE);
        writer.finish(&mut out);
        assert_eq!(crc16(0, &out.0), 0);
        assert_eq!(crc16(0, &out.0[..14]), 0);
        assert_eq!(crc16(0, b"123456789"), 0xbb3d);
    }

    #[test]
    fn header_definition_and_record_layout() {
        let mut out = Bytes(Vec::new());
        let size = (MESSAGE.definition_size() + MESSAGE.record_size()) as u32;
        let mut writer = Writer::start(&mut out, size);
        writer.define(&mut out, &MESSAGE);
        writer.record(
            &mut out,
            &MESSAGE,
            &[
                Value::U32(Some(0x0102_0304)),
                Value::U8(None),
                Value::I32(Some(-2)),
                Value::Str("VANA C606"),
            ],
        );
        writer.finish(&mut out);
        let bytes = &out.0;
        assert_eq!(&bytes[..4], &[14, 0x10, 0x5c, 0x08]);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), size);
        assert_eq!(&bytes[8..12], b".FIT");
        assert_eq!(&bytes[14..20], &[0x43, 0, 0, 20, 0, 4]);
        assert_eq!(
            &bytes[20..32],
            &[253, 4, 0x86, 3, 1, 2, 0, 4, 0x85, 8, 6, 7]
        );
        let record = &bytes[32..32 + MESSAGE.record_size()];
        assert_eq!(
            record,
            &[
                3, 4, 3, 2, 1, 0xff, 0xfe, 0xff, 0xff, 0xff, b'V', b'A', b'N', b'A', b' ', 0
            ]
        );
        assert_eq!(bytes.len(), HEADER_SIZE + size as usize + CRC_SIZE);
        let mut counter = Counter::default();
        let mut counted = Writer::start(&mut counter, 0);
        counted.define(&mut counter, &MESSAGE);
        counted.record(
            &mut counter,
            &MESSAGE,
            &[
                Value::U32(None),
                Value::U8(Some(1)),
                Value::I32(None),
                Value::Str(""),
            ],
        );
        counted.finish(&mut counter);
        assert_eq!(counter.0, bytes.len());
        assert_eq!(counted.written(), bytes.len());
    }

    #[test]
    fn time_and_position_conversions() {
        assert_eq!(timestamp(1_000_000_000_000), Some(368_934_400));
        assert_eq!(timestamp(0), None);
        assert_eq!(timestamp(FIT_EPOCH_UNIX_S * 1000 + 5_000), None);
        assert_eq!(semicircles(900_000_000), 1 << 30);
        assert_eq!(semicircles(-900_000_000), -(1 << 30));
        assert_eq!(semicircles(1_800_000_000), i32::MAX);
        assert_eq!(semicircles(-333_646_900), -398_056_256);
    }

    #[test]
    #[should_panic(expected = "FIT field 3 type")]
    fn mismatched_values_are_rejected() {
        let mut out = Counter::default();
        let mut writer = Writer::start(&mut out, 0);
        writer.record(
            &mut out,
            &MESSAGE,
            &[
                Value::U32(None),
                Value::U16(None),
                Value::I32(None),
                Value::Str(""),
            ],
        );
    }
}
