//! The trace of this power-on session. The handles VANA reads are frozen
//! once a pass and recorded as the simulator records them
//! (`tools/simulator/src/session.rs`), so a trace replays there. Blocks wait
//! in a PSRAM spool and drain into `traces/` on the card's STORE volume
//! (`firmware_services::trace::spool`) while the power owner admits work to
//! the card: one record a pass while a ride records, up to
//! `spool::IDLE_RECORDS` otherwise. The file opens once VANA's boot scans
//! end and the header holds the media they found. Without a STORE
//! volume when the card first admits work, or without PSRAM for the spool,
//! recording is off for the session and says why; nothing else changes.
use c606_firmware as device;
use core::cell::RefCell;
use core::fmt::Write;
use device_api::bulk::ReadWrite;
use device_api::observation::Availability;
use device_api::sensors::Startup;
use firmware_services::trace::frozen::{
    FrozenAccessPoint, FrozenAnt, FrozenBle, FrozenCompanion, FrozenPosition, FrozenPowerControl,
    FrozenServer, FrozenSound, Latest, Record, Recorder,
};
use firmware_services::trace::origin::{self, Config, Files, Media, Memory, Terrain};
use firmware_services::trace::spool::{self, File, Spool, Spooled};
use firmware_services::trace::{Event, redact};

type Bytes = &'static mut [u8];
pub type Recording = Option<Spooled<Bytes, Bytes>>;

/// Spool sizes to try, largest first, each leaving [`RESERVE`] of PSRAM for
/// what allocates later: the transfer session, the phone server's DHCP.
const RINGS: [usize; 3] = [
    spool::RING_BYTES,
    spool::RING_BYTES / 2,
    spool::RING_BYTES / 4,
];
const RESERVE: usize = 64 * 1024;
const BLOCK: usize = 4096;
const CUE: usize = 32;

#[derive(Clone, Copy, Default)]
struct Note {
    pattern: u8,
    cue: [u8; CUE],
    len: usize,
    note: u8,
    of: u8,
}
/// Warnings and errors a pass can hand the trace: a dozen full log lines.
const WARNINGS: usize = 2048;
/// A header, and its config and media encoded first: the settings record
/// and every FILES entry, with room to spare.
const HEADER: usize = 2 * (1024 + origin::FILE_ENTRIES);

/// What the device started from, for the header.
struct Start {
    boot: u32,
    settings: firmware_shell::preferences::Settings,
    memory: Memory,
    files: Option<(u64, u64, usize)>,
    terrain: Option<Terrain>,
    entries: Bytes,
    header: Bytes,
}

pub struct Trace {
    pub record: RefCell<Recording>,
    pub ant: FrozenAnt,
    pub ble: FrozenBle,
    pub position: FrozenPosition,
    pub companion: FrozenCompanion,
    pub startup: Latest<Startup>,
    pub physical: Latest<device_api::sensors::Snapshot>,
    pub network_time: Latest<firmware_services::network_time::Snapshot>,
    pub power: FrozenPowerControl,
    pub backlight: Latest<Availability>,
    pub sound: FrozenSound,
    pub access_point: FrozenAccessPoint,
    pub server: FrozenServer,
    pub shell: &'static firmware_services::trace::shell::ShellLog,
    pub card: core::cell::Cell<u32>,
    file: File<&'static [u8]>,
    start: Option<Start>,
    sector: [u8; device_api::bulk::SECTOR_SIZE],
}

impl Trace {
    /// The trace of a session that just powered on, before its first pass:
    /// it takes its PSRAM; `settings` and `memory` are what the shell and
    /// VANA start with.
    pub fn start(
        boot: u32,
        settings: firmware_shell::preferences::Settings,
        memory: Memory,
        shell: &'static firmware_services::trace::shell::ShellLog,
    ) -> Self {
        let mut trace = Self {
            record: RefCell::new(None),
            ant: FrozenAnt::default(),
            ble: FrozenBle::default(),
            position: FrozenPosition::default(),
            companion: FrozenCompanion::default(),
            startup: Latest::default(),
            physical: Latest::default(),
            network_time: Latest::default(),
            power: FrozenPowerControl::default(),
            backlight: Latest::default(),
            sound: FrozenSound::default(),
            access_point: FrozenAccessPoint::default(),
            server: FrozenServer::default(),
            shell,
            card: core::cell::Cell::new(0),
            file: File::Waiting,
            start: None,
            sector: [0; device_api::bulk::SECTOR_SIZE],
        };
        trace.access_point.redact();
        if let Err(why) = trace.allocate(boot, settings, memory) {
            log::warn!(target: "trace", "off: {why}");
            trace.file = File::Off(why);
        }
        trace
    }

    fn allocate(
        &mut self,
        boot: u32,
        settings: firmware_shell::preferences::Settings,
        memory: Memory,
    ) -> Result<(), &'static str> {
        let entries = device::drivers::psram::zeroed(origin::FILE_ENTRIES).ok_or("no PSRAM")?;
        let header = device::drivers::psram::zeroed(HEADER).ok_or("no PSRAM")?;
        let block = device::drivers::psram::zeroed(BLOCK).ok_or("no PSRAM")?;
        let warnings = device::drivers::psram::zeroed(WARNINGS).ok_or("no PSRAM")?;
        let ring = RINGS
            .into_iter()
            .find_map(|len| {
                if device::drivers::psram::external_free() < len + RESERVE {
                    return None;
                }
                device::drivers::psram::zeroed(len)
            })
            .ok_or("no PSRAM for the spool")?;
        *self.record.get_mut() = Some(Recorder::new(block, Spool::new(ring)));
        crate::logging::keep_warnings(warnings);
        self.start = Some(Start {
            boot,
            settings,
            memory,
            files: None,
            terrain: None,
            entries,
            header,
        });
        Ok(())
    }

    fn stage(&mut self, bulk: &mut impl ReadWrite) {
        self.file = File::stage(bulk, &mut self.sector);
        let (File::Staging(..), Some(start)) = (&self.file, &mut self.start) else {
            if let File::Off(why) = self.file {
                log::warn!(target: "trace", "off: {why}");
            }
            *self.record.get_mut() = None;
            return;
        };
        let (files, terrain) = origin::volumes(bulk, &mut *start.entries, &mut self.sector);
        start.files = files.map(|files| (files.store, files.generation, files.entries.len()));
        start.terrain = terrain;
        log::info!(target: "trace", "staging boot={:08x}", start.boot);
    }

    /// The file is open: a STORE format would erase it.
    pub fn open(&self) -> bool {
        self.file.is_open()
    }

    pub fn record(&self, event: &Event<'_>) {
        self.record.borrow_mut().record(event);
    }

    /// A terminal request line, with its secrets redacted.
    pub fn command(&self, line: &[u8]) {
        if let Ok(text) = core::str::from_utf8(line) {
            let mut redacted = [0u8; 256];
            self.record(&Event::Command(redact::command(text, &mut redacted)));
        }
    }

    /// What the rider saw this pass, as the simulator records it
    /// (`record_seen` in `tools/simulator/src/session.rs`). Not inlined, so
    /// its locals stay out of `vana_pass`'s frame, which stays under every
    /// frame VANA runs.
    #[inline(never)]
    pub fn seen(&mut self, sdk: &mut vana::app::Runtime, now: u64) {
        use vana::app::Seen;
        let mut heard: Option<Note> = None;
        let mut record = self.record.borrow_mut();
        sdk.report_seen(now, &mut |shown| match shown {
            Seen::Page(page) => record.record(&Event::Page(page)),
            Seen::Toast(text) => record.record(&Event::Toast(text)),
            Seen::RideState(state) => record.record(&Event::RideState(state)),
            Seen::Shown(fields) => record.record(&Event::Shown(fields)),
            Seen::Note {
                cue,
                pattern,
                note,
                of,
            } => {
                let len = cue.len().min(CUE);
                let mut name = [0u8; CUE];
                name[..len].copy_from_slice(&cue.as_bytes()[..len]);
                heard = Some(Note {
                    pattern,
                    cue: name,
                    len,
                    note,
                    of,
                });
            }
        });
        drop(record);
        for pattern in self.sound.take_started() {
            let played = heard
                .take_if(|heard| heard.pattern == pattern)
                .unwrap_or_default();
            self.record(&Event::Sound {
                pattern,
                cue: core::str::from_utf8(&played.cue[..played.len]).unwrap_or(""),
                note: played.note,
                of: played.of,
            });
        }
        crate::logging::drain_warnings(|level, target, text| {
            self.record(&Event::Log {
                level,
                target,
                text,
            });
        });
    }

    /// This pass's share of the spool to the trace file while the card
    /// admits work (`admitted`): one record while a ride records (`riding`),
    /// up to `spool::IDLE_RECORDS` otherwise, and everything when VANA asks
    /// to turn off in the next pass (`shutdown_due`).
    pub fn drain(
        &mut self,
        bulk: &mut impl ReadWrite,
        journals: Option<vana::app::Journals>,
        admitted: bool,
        riding: bool,
        shutdown_due: bool,
        now: u64,
    ) {
        if !admitted || self.record.get_mut().is_none() {
            return;
        }
        if let File::Waiting = self.file {
            self.stage(bulk);
        }
        if shutdown_due {
            self.sync(bulk, now);
            return;
        }
        let Some(recorder) = self.record.get_mut() else {
            return;
        };
        let opening = !self.file.is_open();
        let start = &mut self.start;
        let header = || {
            let journals = journals?;
            let start = start.take()?;
            let boot = start.boot;
            Some((start.header(journals)?, boot))
        };
        let records = if riding { 1 } else { spool::IDLE_RECORDS };
        let passed = self
            .file
            .pass(recorder, bulk, true, records, header, now, &mut self.sector);
        if let File::Open(_, _, drain) = &self.file
            && opening
        {
            log::info!(target: "trace", "recording file={}", drain.name().as_str());
        }
        self.ended(passed);
    }

    /// Append everything the spool holds and sync: before a host takes the
    /// card, a restart, or a power transition.
    pub fn sync(&mut self, bulk: &mut impl ReadWrite, now: u64) {
        let Some(recorder) = self.record.get_mut() else {
            return;
        };
        let synced = self.file.sync(recorder, bulk, now, &mut self.sector);
        self.ended(synced);
    }

    /// Before a transfer session; see [`File::before_host`].
    pub fn before_host(&mut self, bulk: &mut impl ReadWrite, now: u64) {
        let Some(recorder) = self.record.get_mut() else {
            return;
        };
        let before = self.file.before_host(recorder, bulk, now, &mut self.sector);
        self.ended(before);
    }

    /// After a transfer session (`unseen`) or a recorded volume command,
    /// which may have changed the card; see [`File::after_host`].
    pub fn after_host(&mut self, bulk: &mut impl ReadWrite, unseen: bool, now: u64) {
        let Some(recorder) = self.record.get_mut() else {
            return;
        };
        let after = self
            .file
            .after_host(recorder, bulk, unseen, now, &mut self.sector);
        self.ended(after);
    }

    fn ended(&mut self, result: Result<(), &'static str>) {
        if let Err(why) = result {
            log::warn!(target: "trace", "ended: {why}");
            *self.record.get_mut() = None;
        }
    }

    /// ` trace=STATE ...` for `STORE STATUS`; a reason's words are joined
    /// by `_`, so every field stays one `key=value` word.
    pub fn status(&self, out: &mut impl Write) {
        let reason = |out: &mut dyn Write, state: &str, why: &str| {
            write!(out, " trace={state} why=")?;
            for (index, word) in why.split(' ').enumerate() {
                write!(out, "{}{word}", if index > 0 { "_" } else { "" })?;
            }
            Ok::<(), core::fmt::Error>(())
        };
        let _ = match &self.file {
            File::Waiting => write!(out, " trace=waiting"),
            File::Off(why) => reason(out, "off", why),
            File::Ended(why) => reason(out, "ended", why),
            File::Staging(..) => write!(out, " trace=staging"),
            File::Open(_, _, drain) => write!(
                out,
                " trace=recording file={} written={}",
                drain.name().as_str(),
                drain.written()
            ),
        };
        if let Some(recorder) = self.record.borrow().as_ref() {
            let spool = recorder.sink();
            let _ = write!(
                out,
                " spool={}/{} spool_peak={} spool_refused={}",
                spool.len(),
                spool.capacity(),
                spool.peak(),
                spool.refused()
            );
        }
    }
}

impl Start {
    /// The header, built into PSRAM, with the saved Wi-Fi password redacted.
    fn header(self, journals: vana::app::Journals) -> Option<&'static [u8]> {
        let mut settings = self.settings;
        if let Some(wifi) = &mut settings.wifi {
            let mut password = [0u8; 63];
            let len = redact::password(wifi.password.bytes(), &mut password);
            wifi.password = device_api::connectivity::Text::new(&password[..len])?;
        }
        let encoded = settings.encode();
        let config = Config {
            settings: &encoded,
            memory: self.memory,
            redacted: true,
        };
        let entries: &'static [u8] = self.entries;
        let media = Media {
            journal: Some(journals.internal),
            rides: journals.bulk,
            files: self.files.map(|(store, generation, len)| Files {
                store,
                generation,
                entries: &entries[..len],
            }),
            terrain: self.terrain,
        };
        let (scratch, out) = self.header.split_at_mut(HEADER / 2);
        let commit = match option_env!("CYCLING_BUILD_DIRTY") {
            Some("1") => "",
            _ => option_env!("CYCLING_BUILD_COMMIT").unwrap_or(""),
        };
        let profile = self.settings.ble_profile;
        let len = origin::header(commit, self.boot, profile, &config, &media, scratch, out)?;
        let out: &'static [u8] = out;
        Some(&out[..len])
    }
}
