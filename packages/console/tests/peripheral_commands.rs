use firmware_console::commands::peripherals::Command;
use firmware_console::commands::peripherals::execute;

// Other capabilities must not be touched by a media command.
struct Untouched;
impl device_api::sound::Sound for Untouched {
    fn patterns(&self) -> &'static [device_api::sound::Pattern] {
        panic!("sound accessed")
    }
    fn snapshot(&self) -> device_api::sound::Snapshot {
        panic!("sound accessed")
    }
    fn play(&mut self, _: u8, _: u64) -> Result<(), device_api::observation::Error> {
        panic!("sound accessed")
    }
    fn stop(&mut self, _: u64) -> Result<(), device_api::observation::Error> {
        panic!("sound accessed")
    }
}
impl device_api::position_control::Control for Untouched {
    fn control(&self) -> device_api::position_control::Snapshot {
        panic!("GNSS accessed")
    }
    fn pause(&mut self, _: u32, _: u64) -> Result<(), device_api::observation::Error> {
        panic!("GNSS accessed")
    }
    fn resume(&mut self, _: u64) -> Result<(), device_api::observation::Error> {
        panic!("GNSS accessed")
    }
}
impl device_api::sensors::Sensors for Untouched {
    fn snapshot(&self, _: u64) -> device_api::sensors::Snapshot {
        panic!("sensors accessed")
    }
    fn identity_status(&self) -> &'static str {
        panic!("sensors accessed")
    }
    fn query_identity(&mut self, _: u64) -> Result<(), device_api::observation::Error> {
        panic!("sensors accessed")
    }
}
struct PartialMedia {
    reads: usize,
    recoveries: usize,
    recovery: Result<(), device_api::bulk::Error>,
}
impl PartialMedia {
    fn new() -> Self {
        Self {
            reads: 0,
            recoveries: 0,
            recovery: Err(device_api::bulk::Error::Failed),
        }
    }
}
impl device_api::bulk::Read for PartialMedia {
    fn info(&self) -> Result<device_api::bulk::Info, device_api::bulk::Error> {
        panic!("failed read must not query success metadata")
    }
    fn read(&mut self, _: u64, output: &mut [u8; 512]) -> Result<(), device_api::bulk::Error> {
        self.reads += 1;
        output[..123].fill(0xab);
        Err(device_api::bulk::Error::Failed)
    }
    fn clock(&mut self, _: u32) -> Result<(), device_api::bulk::Error> {
        panic!("clock changed")
    }
    fn recover(&mut self) -> Result<(), device_api::bulk::Error> {
        self.recoveries += 1;
        self.recovery
    }
}
fn run(
    command: Command,
    media: &mut (impl device_api::bulk::Read + device_api::bulk::ReadWrite),
    output: &mut String,
) -> &'static str {
    execute(
        command,
        0,
        media,
        &mut Untouched,
        &mut Untouched,
        &mut Untouched,
        output,
    )
}

#[test]
fn failed_read_does_not_publish_partial_data_or_success_metadata() {
    let mut media = PartialMedia::new();
    let mut output = String::from("existing reply prefix ");
    assert_eq!(
        run(
            Command::MmcRead {
                sector: 0,
                offset: 0,
                length: 256
            },
            &mut media,
            &mut output
        ),
        "FAILED"
    );
    assert_eq!(media.reads, 1);
    assert_eq!(output, "existing reply prefix ");
}

#[test]
fn invalid_chunk_never_reaches_media() {
    let mut media = PartialMedia::new();
    for (offset, length) in [(0, 0), (0, 257), (511, 2), (u16::MAX, 1)] {
        let mut output = String::new();
        assert_eq!(
            run(
                Command::MmcRead {
                    sector: 0,
                    offset,
                    length
                },
                &mut media,
                &mut output
            ),
            "INVALID"
        );
        assert!(output.is_empty());
    }
    assert_eq!(media.reads, 0);
}

#[test]
fn recovery_failure_cannot_be_reported_as_success() {
    let mut media = PartialMedia::new();
    for (result, expected) in [
        (Err(device_api::bulk::Error::Failed), "FAILED"),
        (Err(device_api::bulk::Error::Unavailable), "UNAVAILABLE"),
        (Err(device_api::bulk::Error::Unsupported), "UNSUPPORTED"),
        (Ok(()), "OK"),
    ] {
        media.recovery = result;
        let mut output = String::new();
        assert_eq!(run(Command::MmcRecover, &mut media, &mut output), expected);
        assert!(output.is_empty());
    }
    assert_eq!(media.recoveries, 4);
    assert_eq!(media.reads, 0);
}

impl device_api::bulk::ReadWrite for PartialMedia {
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        Err(device_api::bulk::Error::Unavailable)
    }
    fn owned_read(&mut self, _: u64, _: &mut [u8; 512]) -> Result<(), device_api::bulk::Error> {
        Err(device_api::bulk::Error::Unavailable)
    }
    fn owned_write(&mut self, _: u64, _: &[u8; 512]) -> Result<(), device_api::bulk::Error> {
        Err(device_api::bulk::Error::Unavailable)
    }
}

struct OwnedMedia {
    sectors: [[u8; 512]; 3],
    reads: Vec<u64>,
    writes: Vec<(u64, [u8; 512])>,
    fail_read: bool,
    fail_write: bool,
}
impl OwnedMedia {
    fn new() -> Self {
        Self {
            sectors: [[0x11; 512], [0x22; 512], [0x33; 512]],
            reads: Vec::new(),
            writes: Vec::new(),
            fail_read: false,
            fail_write: false,
        }
    }
}
impl device_api::bulk::Read for OwnedMedia {
    fn info(&self) -> Result<device_api::bulk::Info, device_api::bulk::Error> {
        panic!("raw info accessed")
    }
    fn read(&mut self, _: u64, _: &mut [u8; 512]) -> Result<(), device_api::bulk::Error> {
        panic!("raw read accessed")
    }
    fn clock(&mut self, _: u32) -> Result<(), device_api::bulk::Error> {
        panic!("raw clock accessed")
    }
    fn recover(&mut self) -> Result<(), device_api::bulk::Error> {
        panic!("raw recover accessed")
    }
}
impl device_api::bulk::ReadWrite for OwnedMedia {
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        Ok(device_api::bulk::OwnedInfo {
            start_sector: 1_955_841,
            sectors: 3,
            sector_size: 512,
        })
    }
    fn owned_read(
        &mut self,
        sector: u64,
        output: &mut [u8; 512],
    ) -> Result<(), device_api::bulk::Error> {
        self.reads.push(sector);
        if self.fail_read {
            output[..123].fill(0xab);
            return Err(device_api::bulk::Error::Failed);
        }
        let index = usize::try_from(sector).map_err(|_| device_api::bulk::Error::Range)?;
        *output = *self
            .sectors
            .get(index)
            .ok_or(device_api::bulk::Error::Range)?;
        Ok(())
    }
    fn owned_write(
        &mut self,
        sector: u64,
        input: &[u8; 512],
    ) -> Result<(), device_api::bulk::Error> {
        self.writes.push((sector, *input));
        let index = usize::try_from(sector).map_err(|_| device_api::bulk::Error::Range)?;
        *self
            .sectors
            .get_mut(index)
            .ok_or(device_api::bulk::Error::Range)? = *input;
        // Model uncertain completion: bytes changed but verification failed.
        if self.fail_write {
            return Err(device_api::bulk::Error::Failed);
        }
        Ok(())
    }
}

#[test]
fn owned_status_and_read_use_only_relative_capability() {
    let mut media = OwnedMedia::new();
    let mut output = String::new();
    assert_eq!(run(Command::MmcOwned, &mut media, &mut output), "OK");
    assert_eq!(
        output,
        "start_sector=1955841 sectors=3 sector_size=512 access=read-write-relative"
    );
    assert!(media.reads.is_empty());
    output.clear();
    assert_eq!(
        run(
            Command::MmcOwnedRead {
                sector: 1,
                offset: 510,
                length: 2
            },
            &mut media,
            &mut output
        ),
        "OK"
    );
    assert_eq!(
        output,
        format!(
            "sector=1 offset=510 length=2 crc32={:08x} data=2222",
            firmware_services::bulk::crc32(&[0x22; 2])
        )
    );
    assert_eq!(media.reads, [1]);
    assert!(media.writes.is_empty());
}

#[test]
fn owned_read_failure_never_publishes_partial_bytes() {
    let mut media = OwnedMedia::new();
    media.fail_read = true;
    let mut output = String::from("existing reply prefix ");
    assert_eq!(
        run(
            Command::MmcOwnedRead {
                sector: 1,
                offset: 0,
                length: 123
            },
            &mut media,
            &mut output
        ),
        "FAILED"
    );
    assert_eq!(output, "existing reply prefix ");
    assert_eq!(media.reads, [1]);
    assert!(media.writes.is_empty());
}

#[test]
fn owned_read_rejects_bad_chunks_before_access() {
    let mut media = OwnedMedia::new();
    for (offset, length) in [(0, 0), (0, 257), (511, 2), (u16::MAX, 1)] {
        let mut output = String::new();
        assert_eq!(
            run(
                Command::MmcOwnedRead {
                    sector: 1,
                    offset,
                    length
                },
                &mut media,
                &mut output
            ),
            "INVALID"
        );
        assert!(output.is_empty());
    }
    assert!(media.reads.is_empty());
    assert!(media.writes.is_empty());
}

#[test]
fn owned_commands_parse_and_test_requires_harness() {
    use firmware_console::commands::peripherals::parse;
    assert_eq!(
        parse("MMC", &mut "OWNED STATUS".split_ascii_whitespace()),
        Some(Command::MmcOwned)
    );
    assert_eq!(
        parse("MMC", &mut "OWNED READ 1 510 2".split_ascii_whitespace()),
        Some(Command::MmcOwnedRead {
            sector: 1,
            offset: 510,
            length: 2
        })
    );
    let parsed = parse(
        "MMC",
        &mut "OWNED TEST 1 deadbeef a5".split_ascii_whitespace(),
    );
    #[cfg(not(feature = "debug-harness"))]
    assert_eq!(parsed, None);
    #[cfg(feature = "debug-harness")]
    assert_eq!(
        parsed,
        Some(Command::MmcOwnedTest {
            sector: 1,
            expected_crc: 0xdeadbeef,
            fill: 0xa5
        })
    );
}

#[cfg(feature = "debug-harness")]
#[test]
fn owned_test_conflict_never_writes() {
    let mut media = OwnedMedia::new();
    let before = media.sectors;
    let expected_crc = firmware_services::bulk::crc32(&media.sectors[1]) ^ 1;
    let mut output = String::new();
    assert_eq!(
        run(
            Command::MmcOwnedTest {
                sector: 1,
                expected_crc,
                fill: 0xa5
            },
            &mut media,
            &mut output
        ),
        "CONFLICT"
    );
    assert_eq!(media.reads, [1]);
    assert!(media.writes.is_empty());
    assert_eq!(media.sectors, before);
    assert!(output.is_empty());
}

#[cfg(feature = "debug-harness")]
#[test]
fn owned_test_writes_one_requested_relative_sector_with_requested_fill() {
    let mut media = OwnedMedia::new();
    let before = media.sectors;
    let expected_crc = firmware_services::bulk::crc32(&media.sectors[1]);
    let mut output = String::new();
    assert_eq!(
        run(
            Command::MmcOwnedTest {
                sector: 1,
                expected_crc,
                fill: 0xa5
            },
            &mut media,
            &mut output
        ),
        "OK"
    );
    assert_eq!(media.reads, [1]);
    assert_eq!(media.writes, [(1, [0xa5; 512])]);
    assert_eq!(media.sectors, [before[0], [0xa5; 512], before[2]]);
    assert_eq!(
        output,
        format!(
            "sector=1 length=512 crc32={:08x} verified=true",
            firmware_services::bulk::crc32(&[0xa5; 512])
        )
    );
}

#[cfg(feature = "debug-harness")]
#[test]
fn owned_test_failed_read_cannot_mutate() {
    let mut media = OwnedMedia::new();
    let before = media.sectors;
    media.fail_read = true;
    let expected_crc = firmware_services::bulk::crc32(&media.sectors[1]);
    let mut output = String::from("existing reply prefix ");
    assert_eq!(
        run(
            Command::MmcOwnedTest {
                sector: 1,
                expected_crc,
                fill: 0xa5
            },
            &mut media,
            &mut output
        ),
        "FAILED"
    );
    assert_eq!(media.reads, [1]);
    assert!(media.writes.is_empty());
    assert_eq!(media.sectors, before);
    assert_eq!(output, "existing reply prefix ");
}

#[cfg(feature = "debug-harness")]
#[test]
fn owned_test_ambiguous_write_is_not_retried_or_reported_successful() {
    let mut media = OwnedMedia::new();
    media.fail_write = true;
    let expected_crc = firmware_services::bulk::crc32(&media.sectors[1]);
    let mut output = String::from("existing reply prefix ");
    assert_eq!(
        run(
            Command::MmcOwnedTest {
                sector: 1,
                expected_crc,
                fill: 0xa5
            },
            &mut media,
            &mut output
        ),
        "FAILED"
    );
    assert_eq!(media.reads, [1]);
    assert_eq!(media.writes, [(1, [0xa5; 512])]);
    assert_eq!(output, "existing reply prefix ");
}
