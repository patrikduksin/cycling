//! A two-sector, power-loss-tolerant journal for small settings records.

pub const SECTOR_SIZE: usize = 4096;
pub const SECTOR_COUNT: usize = 2;
pub const HEADER_SIZE: usize = 24;
pub const CAPACITY: usize = SECTOR_SIZE - HEADER_SIZE;

const MAGIC: [u8; 4] = *b"C606";
const FORMAT_VERSION: u16 = 1;
const COMMITTED: u32 = 0x434f_4d54;
const COMMIT_OFFSET: usize = 20;

#[repr(C, align(4))]
pub struct Sector(pub [u8; SECTOR_SIZE]);

impl Default for Sector {
    fn default() -> Self {
        Self([0; SECTOR_SIZE])
    }
}

pub trait Flash {
    type Error;

    fn read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), Self::Error>;
    fn erase_sector(&mut self, sector: usize) -> Result<(), Self::Error>;
    fn write_sector(&mut self, sector: usize, data: &Sector) -> Result<(), Self::Error>;
    fn commit_sector(&mut self, sector: usize, commit: &[u8; 4]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record {
    pub sequence: u32,
    pub length: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error<E> {
    Flash(E),
    PayloadTooLarge,
    OutputTooSmall,
    ReadbackFailed,
}

pub struct Journal<F> {
    flash: F,
    scratch: Sector,
}

impl<F: Flash> Journal<F> {
    pub fn new(flash: F) -> Self {
        Self {
            flash,
            scratch: Sector::default(),
        }
    }

    pub fn load(&mut self, output: &mut [u8]) -> Result<Option<Record>, Error<F::Error>> {
        let newest = self.scan()?;
        let Some((sector, record)) = newest else {
            return Ok(None);
        };
        if output.len() < record.length {
            return Err(Error::OutputTooSmall);
        }
        self.flash
            .read_sector(sector, &mut self.scratch)
            .map_err(Error::Flash)?;
        let verified = decode(&self.scratch).ok_or(Error::ReadbackFailed)?;
        if verified != record {
            return Err(Error::ReadbackFailed);
        }
        output[..verified.length]
            .copy_from_slice(&self.scratch.0[HEADER_SIZE..HEADER_SIZE + verified.length]);
        Ok(Some(verified))
    }

    pub fn save(&mut self, payload: &[u8]) -> Result<Record, Error<F::Error>> {
        if payload.len() > CAPACITY {
            return Err(Error::PayloadTooLarge);
        }
        let newest = self.scan()?;
        let (target, sequence) = match newest {
            Some((sector, record)) => (1 - sector, record.sequence.wrapping_add(1)),
            None => (0, 1),
        };

        self.scratch.0.fill(0xff);
        self.scratch.0[0..4].copy_from_slice(&MAGIC);
        put_u16(&mut self.scratch.0, 4, FORMAT_VERSION);
        put_u16(&mut self.scratch.0, 6, HEADER_SIZE as u16);
        put_u32(&mut self.scratch.0, 8, sequence);
        put_u32(&mut self.scratch.0, 12, payload.len() as u32);
        self.scratch.0[HEADER_SIZE..HEADER_SIZE + payload.len()].copy_from_slice(payload);
        let checksum = checksum(sequence, payload);
        put_u32(&mut self.scratch.0, 16, checksum);

        self.flash.erase_sector(target).map_err(Error::Flash)?;
        self.flash
            .write_sector(target, &self.scratch)
            .map_err(Error::Flash)?;
        self.flash
            .commit_sector(target, &COMMITTED.to_le_bytes())
            .map_err(Error::Flash)?;

        let mut verify = Sector::default();
        self.flash
            .read_sector(target, &mut verify)
            .map_err(Error::Flash)?;
        let record = decode(&verify).ok_or(Error::ReadbackFailed)?;
        if record.sequence != sequence
            || record.length != payload.len()
            || verify.0[HEADER_SIZE..HEADER_SIZE + payload.len()] != *payload
        {
            return Err(Error::ReadbackFailed);
        }
        Ok(record)
    }

    fn scan(&mut self) -> Result<Option<(usize, Record)>, Error<F::Error>> {
        let mut newest = None;
        for sector in 0..SECTOR_COUNT {
            self.flash
                .read_sector(sector, &mut self.scratch)
                .map_err(Error::Flash)?;
            if let Some(record) = decode(&self.scratch)
                && newest
                    .map(|(_, current): (usize, Record)| newer(record.sequence, current.sequence))
                    .unwrap_or(true)
            {
                newest = Some((sector, record));
            }
        }
        Ok(newest)
    }
}

fn decode(sector: &Sector) -> Option<Record> {
    let bytes = &sector.0;
    let length = get_u32(bytes, 12) as usize;
    if bytes[0..4] != MAGIC
        || get_u16(bytes, 4) != FORMAT_VERSION
        || get_u16(bytes, 6) as usize != HEADER_SIZE
        || length > CAPACITY
        || get_u32(bytes, COMMIT_OFFSET) != COMMITTED
    {
        return None;
    }
    let sequence = get_u32(bytes, 8);
    let payload = &bytes[HEADER_SIZE..HEADER_SIZE + length];
    (get_u32(bytes, 16) == checksum(sequence, payload)).then_some(Record { sequence, length })
}

fn newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) < (1 << 31)
}

fn checksum(sequence: u32, payload: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;
    for byte in sequence
        .to_le_bytes()
        .into_iter()
        .chain((payload.len() as u32).to_le_bytes())
        .chain(payload.iter().copied())
    {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Memory {
        bytes: [[u8; SECTOR_SIZE]; SECTOR_COUNT],
        fail_after: Option<usize>,
        tear_bytes: usize,
        operations: usize,
    }

    impl Default for Memory {
        fn default() -> Self {
            Self {
                bytes: [[0xff; SECTOR_SIZE]; SECTOR_COUNT],
                fail_after: None,
                tear_bytes: 0,
                operations: 0,
            }
        }
    }

    impl Memory {
        fn fails(&mut self) -> bool {
            if self.fail_after == Some(self.operations) {
                return true;
            }
            self.operations += 1;
            false
        }
    }

    impl Flash for Memory {
        type Error = ();

        fn read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), Self::Error> {
            if self.fails() {
                return Err(());
            }
            output.0.copy_from_slice(&self.bytes[sector]);
            Ok(())
        }

        fn erase_sector(&mut self, sector: usize) -> Result<(), Self::Error> {
            let fails = self.fails();
            let length = if fails {
                self.tear_bytes.min(SECTOR_SIZE)
            } else {
                SECTOR_SIZE
            };
            self.bytes[sector][..length].fill(0xff);
            if fails { Err(()) } else { Ok(()) }
        }

        fn write_sector(&mut self, sector: usize, data: &Sector) -> Result<(), Self::Error> {
            let fails = self.fails();
            let length = if fails {
                self.tear_bytes.min(SECTOR_SIZE)
            } else {
                SECTOR_SIZE
            };
            for (stored, new) in self.bytes[sector][..length].iter_mut().zip(data.0) {
                *stored &= new;
            }
            if fails { Err(()) } else { Ok(()) }
        }

        fn commit_sector(&mut self, sector: usize, commit: &[u8; 4]) -> Result<(), Self::Error> {
            let fails = self.fails();
            let length = if fails { self.tear_bytes.min(4) } else { 4 };
            for (stored, new) in self.bytes[sector][COMMIT_OFFSET..COMMIT_OFFSET + 4]
                .iter_mut()
                .zip(commit)
                .take(length)
            {
                *stored &= *new;
            }
            if fails { Err(()) } else { Ok(()) }
        }
    }

    #[test]
    fn saves_reads_and_uses_full_capacity() {
        let mut journal = Journal::new(Memory::default());
        let payload = [0xa5; CAPACITY];
        assert_eq!(journal.save(&payload).unwrap().sequence, 1);
        let mut output = [0; CAPACITY];
        assert_eq!(journal.load(&mut output).unwrap().unwrap().length, CAPACITY);
        assert_eq!(output, payload);
        assert_eq!(journal.save(&[1, 2, 3]).unwrap().sequence, 2);
    }

    #[test]
    fn rejects_oversize_and_short_output() {
        let mut journal = Journal::new(Memory::default());
        assert_eq!(
            journal.save(&[0; CAPACITY + 1]),
            Err(Error::PayloadTooLarge)
        );
        journal.save(&[1, 2, 3]).unwrap();
        assert_eq!(journal.load(&mut [0; 2]), Err(Error::OutputTooSmall));
    }

    #[test]
    fn checksum_corruption_falls_back_to_previous_record() {
        let mut journal = Journal::new(Memory::default());
        journal.save(b"old").unwrap();
        journal.save(b"new").unwrap();
        journal.flash.bytes[1][HEADER_SIZE] ^= 1;
        let mut output = [0; 8];
        let record = journal.load(&mut output).unwrap().unwrap();
        assert_eq!(record.sequence, 1);
        assert_eq!(&output[..record.length], b"old");
    }

    #[test]
    fn interruption_before_commit_keeps_previous_record() {
        for failed_operation in 2..5 {
            for tear_bytes in [0, 1, 2, 3, 16, 2048, 4095] {
                let mut journal = Journal::new(Memory::default());
                journal.save(b"older").unwrap();
                journal.save(b"old").unwrap();
                journal.flash.operations = 0;
                journal.flash.fail_after = Some(failed_operation);
                journal.flash.tear_bytes = tear_bytes;
                assert!(journal.save(b"new").is_err());
                journal.flash.fail_after = None;
                let mut output = [0; 8];
                let record = journal.load(&mut output).unwrap().unwrap();
                let recovered = &output[..record.length];
                assert!(
                    recovered == b"old" || recovered == b"new",
                    "failure at operation {failed_operation}, byte {tear_bytes}"
                );
            }
        }
    }
}
