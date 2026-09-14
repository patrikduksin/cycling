use cycling_os::{
    bulk, capabilities, companion_sensors,
    peripheral_commands::{Command, execute},
    position_control, sound,
};

// Other capabilities must not be touched by a media command.
struct Untouched;
impl sound::Sound for Untouched {
    fn patterns(&self) -> &'static [sound::Pattern] {
        panic!("sound accessed")
    }
    fn snapshot(&self) -> sound::Snapshot {
        panic!("sound accessed")
    }
    fn play(&mut self, _: u8, _: u64) -> Result<(), capabilities::Error> {
        panic!("sound accessed")
    }
    fn stop(&mut self, _: u64) -> Result<(), capabilities::Error> {
        panic!("sound accessed")
    }
}
impl position_control::Control for Untouched {
    fn control(&self) -> position_control::Snapshot {
        panic!("GNSS accessed")
    }
    fn pause(&mut self, _: u32, _: u64) -> Result<(), capabilities::Error> {
        panic!("GNSS accessed")
    }
    fn resume(&mut self, _: u64) -> Result<(), capabilities::Error> {
        panic!("GNSS accessed")
    }
}
impl capabilities::Sensors for Untouched {
    fn snapshot(&self, _: u64) -> companion_sensors::Snapshot {
        panic!("sensors accessed")
    }
    fn identity_status(&self) -> &'static str {
        panic!("sensors accessed")
    }
    fn query_identity(&mut self, _: u64) -> Result<(), capabilities::Error> {
        panic!("sensors accessed")
    }
}
struct PartialMedia {
    reads: usize,
    recoveries: usize,
    recovery: Result<(), bulk::Error>,
}
impl PartialMedia {
    fn new() -> Self {
        Self {
            reads: 0,
            recoveries: 0,
            recovery: Err(bulk::Error::Failed),
        }
    }
}
impl bulk::Read for PartialMedia {
    fn info(&self) -> Result<bulk::Info, bulk::Error> {
        panic!("failed read must not query success metadata")
    }
    fn read(&mut self, _: u64, output: &mut [u8; 512]) -> Result<(), bulk::Error> {
        self.reads += 1;
        output[..123].fill(0xab);
        Err(bulk::Error::Failed)
    }
    fn clock(&mut self, _: u32) -> Result<(), bulk::Error> {
        panic!("clock changed")
    }
    fn recover(&mut self) -> Result<(), bulk::Error> {
        self.recoveries += 1;
        self.recovery
    }
}
fn run(command: Command, media: &mut PartialMedia, output: &mut String) -> &'static str {
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
        (Err(bulk::Error::Failed), "FAILED"),
        (Err(bulk::Error::Unavailable), "UNAVAILABLE"),
        (Err(bulk::Error::Unsupported), "UNSUPPORTED"),
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
