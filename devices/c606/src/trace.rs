//! The trace of this power-on session: the world log every recorder of
//! the composition writes to (`firmware_services::trace::log`), as the
//! simulator's session writes it (`tools/simulator/src/session.rs`), so a
//! trace replays there. Blocks wait in a PSRAM spool and drain into
//! `traces/` on the card's STORE volume (`firmware_services::trace::spool`)
//! while the power owner admits work to the card: one record a pass while a
//! ride records, up to `spool::IDLE_RECORDS` otherwise. The file opens at
//! the first pass the card admits work. Without a STORE volume then, or
//! without PSRAM for the spool, recording is off for the session and says
//! why; nothing else changes. A session that stops recording, off or
//! ended, gives the spool's PSRAM back: the spool takes it from power-on,
//! before the card says whether it can record, so that the boot and the
//! passes before then are kept.
//!
//! After a panic or a watchdog reset, PSRAM still holds the last session's
//! spool and open block (`drivers::psram`). Once the card admits work, and
//! before this session's file opens, the trace appends what they hold to
//! the trace before (`firmware_services::trace::tail`), records `Salvaged`,
//! and gives that PSRAM to the heap. In a pass after its file opens, every
//! session records the `Horizon` of the trace before.
use c606_firmware as device;
use core::cell::RefCell;
use core::fmt::Write;
use device::drivers::external::Owned;
use device::drivers::psram;
use device_api::bulk::ReadWrite;
use device_api::journal::Latest;
use firmware_services::trace::log::Log;
use firmware_services::trace::media::{Memory as Media, Volumes};
use firmware_services::trace::origin::{self, Found, Memory, Terrain};
use firmware_services::trace::recorder::Recorder;
use firmware_services::trace::spool::{self, Card, File, Spool};
use firmware_services::trace::tail;
use firmware_services::trace::{
    Activity, Consumers, Event, FrameTime, Gnss, Health, Vitals, redact,
};

type Bytes = &'static mut [u8];
pub type WorldLog = Log<Owned, Spool<Owned>, &'static mut Media>;
/// The log as every recorder holds it.
pub type Journal = &'static RefCell<WorldLog>;
/// What a recorder keeps between passes.
pub type Kept<T> = &'static RefCell<T>;

/// What the recorders keep between passes: those bound each pass, then
/// those the shell owns.
#[derive(Default)]
pub struct Observed {
    pub ant: RefCell<device_api::ant::AntObserved>,
    pub ble: RefCell<device_api::ble_transport::BleObserved>,
    pub bulk: RefCell<device_api::bulk::ReadWriteObserved>,
    pub access_point: RefCell<device_api::server::AccessPointObserved>,
    pub server: RefCell<device_api::server::ServerObserved>,
    pub sound: RefCell<device_api::sound::SoundObserved>,
    pub control: RefCell<device_api::power::PowerControlObserved>,
    pub position: RefCell<device_api::positioning::PositioningObserved>,
    pub companion: RefCell<device_api::input::InputObservationObserved>,
    pub sensors: RefCell<device_api::sensors::SensorsObserved>,
    pub wifi: RefCell<device_api::connectivity::WifiObserved>,
    pub display: RefCell<device_api::display::DisplayObserved>,
    pub input: RefCell<device_api::input::InputSourceObserved>,
    pub backlight: RefCell<device_api::power::PowerObserved>,
    pub flash: RefCell<device_api::storage::OwnedFlashObserved>,
    pub settings: RefCell<firmware_shell::storage::SettingsObserved>,
}

/// Spool sizes to try, largest first, each leaving [`RESERVE`] of PSRAM for
/// what allocates later: VANA's runtime, the transfer session, the phone
/// server's DHCP.
const RINGS: [usize; 3] = [
    spool::RING_BYTES,
    spool::RING_BYTES / 2,
    spool::RING_BYTES / 4,
];
const RESERVE: usize = 64 * 1024 + core::mem::size_of::<vana::app::Runtime>();
const CUE_NAME: usize = 32;

#[derive(Clone, Copy, Default)]
struct Note {
    pattern: u8,
    cue: [u8; CUE_NAME],
    len: usize,
    note: u8,
    of: u8,
}
/// Warnings and errors a pass can hand the trace: a dozen full log lines.
const WARNINGS: usize = 2048;
const HEADER: usize = 256;

const MEDIA: Media = Media::with_crc(esp_hal::rom::crc::crc32_le);

struct Start {
    boot: u32,
    terrain: Found<Terrain>,
    header: Bytes,
}

pub struct Trace {
    pub log: Journal,
    pub observed: &'static Observed,
    network_time: Latest<firmware_services::network_time::Snapshot>,
    /// The power status the last pass or command read.
    pub status: device_api::power::Status,
    /// The patterns the buzzer accepted in this pass.
    pub started: heapless::Vec<u8, 4>,
    file: File,
    start: Option<Start>,
    /// What a panic or a watchdog reset left of the session before, until
    /// the card admits work.
    kept: Option<psram::Kept>,
    sector: [u8; device_api::bulk::SECTOR_SIZE],
    vitals: Vitals,
    /// When the current activity window began, and the frames sent then.
    window_ms: u64,
    frames: u32,
}

impl Trace {
    /// The trace of a session that just powered on, before its shell
    /// loads the settings: it takes its PSRAM, replaces secrets from its
    /// first record, and opens with the working `memory` VANA starts with.
    pub fn start(boot: u32, memory: Memory) -> Self {
        let media = psram::place(MEDIA).expect("PSRAM exhausted");
        let (recording, start, file) = match Self::allocate(boot) {
            Ok((recorder, start)) => (Some(recorder), Some(start), File::Waiting),
            Err(why) => {
                log::warn!(target: "trace", "off: {why}");
                (None, None, File::Off(why))
            }
        };
        let mut log = Log::new(recording, media);
        log.redact();
        log.open(memory);
        Self {
            log: crate::external(|| RefCell::new(log)),
            observed: crate::external(Observed::default),
            network_time: Latest::default(),
            status: device_api::power::Status::IDLE,
            started: heapless::Vec::new(),
            file,
            start,
            kept: psram::take_kept(),
            sector: [0; device_api::bulk::SECTOR_SIZE],
            vitals: Vitals::default(),
            window_ms: 0,
            frames: 0,
        }
    }

    fn allocate(boot: u32) -> Result<(Recorder<Owned, Spool<Owned>>, Start), &'static str> {
        let header = psram::zeroed(HEADER).ok_or("no PSRAM")?;
        let block = psram::owned(tail::BLOCK).ok_or("no PSRAM")?;
        let warnings = psram::zeroed(WARNINGS).ok_or("no PSRAM")?;
        let ring = RINGS
            .into_iter()
            .find_map(|len| {
                if psram::external_free() < len + RESERVE {
                    return None;
                }
                psram::owned(len)
            })
            .ok_or("no PSRAM for the spool")?;
        crate::logging::keep_warnings(warnings);
        psram::keep(ring.as_ref(), block.as_ref());
        let start = Start {
            boot,
            terrain: Found::Absent,
            header,
        };
        Ok((Recorder::new(block, Spool::new(ring)), start))
    }

    /// A pass begins; a `quiet` one is recorded only when something else is
    /// recorded in it.
    pub fn begin_pass(&self, now: u64, quiet: bool) {
        self.log.borrow_mut().begin_pass(now, quiet);
    }

    pub fn record(&self, event: &Event<'_>) {
        self.log.borrow_mut().record(event);
    }

    /// A terminal request line, with its secrets redacted: what the
    /// command reads is recorded after it.
    pub fn command(&self, line: &[u8]) {
        let mut log = self.log.borrow_mut();
        if let Ok(text) = core::str::from_utf8(line) {
            let mut redacted = [0u8; 256];
            log.record(&Event::Command(redact::command(text, &mut redacted)));
        }
        log.begin_command();
    }

    /// UTC as this pass reads it, recorded when it no longer reads as the
    /// last recorded value advanced to this pass.
    pub fn network_time(&mut self, snapshot: firmware_services::network_time::Snapshot, now: u64) {
        if self.network_time.changed(snapshot, now) {
            self.record(&Event::NetworkTime(snapshot));
        }
    }

    /// Whether the session still waits for the card's first open pass.
    pub fn waiting(&self) -> bool {
        matches!(self.file, File::Waiting)
    }

    /// At the first pass the card admits work, before VANA reads it: stage
    /// the file on `bulk`, unrecorded, and note the TERRAIN pack for the
    /// header. The caller then reads the volume table for the log.
    #[inline(never)]
    pub fn stage(&mut self, bulk: &mut impl ReadWrite) {
        self.file = File::stage(bulk, &mut self.sector);
        let (File::Staging(..), Some(start)) = (&self.file, &mut self.start) else {
            if let File::Off(why) = self.file {
                log::warn!(target: "trace", "off: {why}");
            }
            Self::off(&mut self.log.borrow_mut());
            return;
        };
        start.terrain = origin::terrain(bulk, &mut self.sector);
        log::info!(target: "trace", "staging boot={:08x}", start.boot);
    }

    /// Tell the log which card sectors are TERRAIN and STORE, from the
    /// volume table as `media` reads it: the read is recorded, so a replay
    /// finds the same volumes.
    #[inline(never)]
    pub fn read_volumes(&mut self, media: &mut impl ReadWrite) {
        let volumes = Volumes::read(media, &mut self.sector);
        self.log.borrow_mut().volumes(volumes);
    }

    /// The file is open: a STORE format would erase it.
    pub fn open(&self) -> bool {
        self.file.is_open()
    }

    /// What the rider saw this pass, as the simulator records it
    /// (`record_seen` in `tools/simulator/src/session.rs`). Not inlined, so
    /// its locals stay out of `traced_pass`'s frame, which stays under every
    /// frame VANA runs.
    #[inline(never)]
    pub fn seen(&mut self, sdk: &mut vana::app::Runtime, frame: Option<u32>, now: u64) {
        use vana::app::Seen;
        let mut heard: Option<Note> = None;
        let mut log = self.log.borrow_mut();
        sdk.report_seen(now, &mut |shown| match shown {
            Seen::Page(page) => log.record(&Event::Page(page)),
            Seen::Toast(text) => log.record(&Event::Toast(text)),
            Seen::RideState(state) => log.record(&Event::RideState(state)),
            Seen::Shown(fields) => log.record(&Event::Shown(fields)),
            Seen::Mark {
                count,
                pressed_ms,
                page,
            } => log.record(&Event::Mark {
                count,
                pressed_ms,
                page,
            }),
            Seen::Note {
                cue,
                pattern,
                note,
                of,
            } => {
                let len = cue.len().min(CUE_NAME);
                let mut name = [0u8; CUE_NAME];
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
        for pattern in core::mem::take(&mut self.started) {
            let played = heard
                .take_if(|heard| heard.pattern == pattern)
                .unwrap_or_default();
            log.record(&Event::Sound {
                pattern,
                cue: core::str::from_utf8(&played.cue[..played.len]).unwrap_or(""),
                note: played.note,
                of: played.of,
            });
        }
        if let Some(digest) = frame {
            log.record(&Event::Panel(digest));
        }
        crate::logging::drain_warnings(|level, target, text| {
            log.record(&Event::Log {
                level,
                target,
                text,
            });
        });
    }

    /// Health and the activity window since the last, when they are due
    /// (see [`Vitals`]). Not inlined, so the counters taken and the records
    /// stay out of the console task's frame.
    #[inline(never)]
    pub fn vitals(
        &mut self,
        world: &crate::world::World,
        heap_min: usize,
        now: u64,
        quiet: bool,
        map_work: impl FnOnce() -> Option<vana::app::MapWork>,
    ) {
        use device_api::ant::Ant as _;
        use device_api::ble_transport::Ble as _;
        use device_api::position_control::State;
        let mut log = self.log.borrow_mut();
        if !self.vitals.due(now, quiet) || log.recorder.is_none() {
            return;
        }
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let position = device::capabilities::positioning::snapshot(now);
        let gps = position.map(|position| position.gps);
        let input = device::capabilities::io::snapshot(now);
        let (spool_used, spool_size) = log.recorder.as_ref().map_or((0, 0), |recorder| {
            (recorder.sink().len(), recorder.sink().capacity())
        });
        let (window_frames, window_total_us) = device::drivers::display::TIMING.take_window_total();
        let health = Health {
            simulated: false,
            heap_free: Some(count(esp_alloc::HEAP.free())),
            heap_min: Some(count(heap_min)),
            psram_free: Some(count(psram::external_free())),
            frame_max_us: Some(device::drivers::display::TIMING.take_window_max()),
            pass_max_us: Some(device::activity::take_pass_max_us()),
            gps_sentences: gps.map_or(0, |gps| gps.valid_sentences),
            gps_overflows: gps.map_or(0, |gps| gps.overflows),
            gps_uart_errors: gps.map_or(0, |gps| gps.uart_errors),
            companion_errors: input.map_or(0, |input| {
                input
                    .companion_bad_crc
                    .saturating_add(input.uart_errors)
                    .saturating_add(input.input_lost)
            }),
            card_write_max_us: Some(device::drivers::sdmmc::HEALTH.take_write_max_us()),
            card_recoveries: Some(device::drivers::sdmmc::HEALTH.recoveries()),
            spool_used: count(spool_used),
            spool_size: count(spool_size),
            wifi: Some(device::drivers::wifi::state()),
            wifi_rssi: device::drivers::wifi::rssi(),
            ble_rssi: device::drivers::bluetooth::target_rssi(),
        };
        let (taken, hooks_us) = device::activity::take();
        let frames = device::drivers::display::TIMING.frames();
        let window_ms = now - core::mem::replace(&mut self.window_ms, now);
        let sent = frames.wrapping_sub(core::mem::replace(&mut self.frames, frames));
        let ant_channels = world
            .ant()
            .channels(now)
            .iter()
            .flatten()
            .filter(|channel| channel.link == device_api::ant::LinkState::Connected)
            .count();
        let gnss = match (
            position,
            device::capabilities::position_control::snapshot().state,
        ) {
            (None, _) => Gnss::Off,
            (_, State::Receiving | State::ResumeQueued | State::ResumeSubmitted) => Gnss::Receiving,
            (_, State::Failed) => Gnss::Failed,
            (_, State::PauseQueued | State::PauseSubmitted | State::SilenceObserved) => {
                Gnss::Paused
            }
        };
        let activity = Activity {
            simulated: false,
            window_ms: u32::try_from(window_ms).unwrap_or(u32::MAX),
            wakes: taken.wakes,
            awake_us: Some(taken.awake_us),
            main_us: Some(taken.main_us),
            radio_us: Some(taken.radio_us),
            idle_us: Some(taken.idle_us),
            switches: Some(taken.switches),
            ticks: Some(taken.ticks),
            hooks_us: Some(hooks_us),
            consumers: Consumers {
                cpu_mhz: u16::try_from(esp_hal::clock::cpu_clock().as_mhz()).ok(),
                backlight: device::capabilities::power::lit(),
                frames: sent,
                gnss,
                wifi: Some(device::drivers::wifi::state()),
                access_point: device::drivers::wifi::access_point_state(),
                ble: world.ble().snapshot().link,
                ant: world.ant().availability(),
                ant_scanning: world.ant().scanning(),
                ant_channels: u8::try_from(ant_channels).unwrap_or(u8::MAX),
                slept_ms: device::drivers::sleep::take_slept_ms(),
            },
        };
        log.cost();
        if let Some(frame_time) = FrameTime::of_window(window_frames, u64::from(window_total_us)) {
            log.record(&Event::FrameTime(frame_time));
        }
        log.record(&Event::Health(health));
        log.record(&Event::Activity(activity));
        for task in taken.tasks.iter().flatten() {
            log.record(&Event::Task {
                name: task.task.map_or("task", device::activity::Task::name),
                polls: task.polls,
                busy_us: Some(task.busy_us),
            });
        }
        for stage in device::activity::take_stages().iter().flatten() {
            log.record(&Event::Stage {
                name: stage.stage.name(),
                runs: stage.runs,
                busy_us: stage.busy_us,
                max_us: stage.max_us,
            });
        }
        if let Some(work) = map_work() {
            for (name, timed) in [
                ("vana.present.map_fill", work.fill),
                ("look_ahead.map_extract", work.extract),
            ] {
                if timed.runs > 0 {
                    log.record(&Event::Stage {
                        name,
                        runs: timed.runs,
                        busy_us: timed.busy_us,
                        max_us: timed.max_us,
                    });
                }
            }
        }
    }

    /// This pass's share of the spool to the trace file, as `card` allows:
    /// the salvage first, then the pass, then the horizon. Not inlined, so
    /// its locals stay out of the console task's frame.
    #[inline(never)]
    pub fn drain(&mut self, bulk: &mut impl ReadWrite, card: Card, now: u64) {
        if card == Card::Closed {
            return;
        }
        if self.kept.is_some() && !self.waiting() {
            self.salvage(bulk);
        }
        let opened = self.file.is_open();
        self.pass(bulk, card, now);
        if opened && self.file.horizon_due() {
            self.horizon(bulk);
        }
    }

    /// After a panic or a watchdog reset, while the file stages: append
    /// what the crash left to the trace before. Then, staging or not, the
    /// kept PSRAM goes to the heap.
    #[inline(never)]
    fn salvage(&mut self, bulk: &mut impl ReadWrite) {
        let Some(kept) = self.kept.take() else {
            return;
        };
        let staging = matches!(self.file, File::Staging(..));
        let window = staging.then(|| psram::owned(tail::WINDOW)).flatten();
        if staging && window.is_none() {
            log::warn!(target: "trace", "salvage failed: no PSRAM");
        }
        if let Some(mut window) = window {
            match self
                .file
                .salvage(bulk, remains(&kept), window.as_mut(), &mut self.sector)
            {
                Ok(blocks) => {
                    log::info!(target: "trace", "salvaged blocks={blocks}");
                    self.log.borrow_mut().record(&Event::Salvaged { blocks });
                }
                Err(why) => log::warn!(target: "trace", "salvage failed: {why}"),
            }
        }
        psram::release(kept);
    }

    /// The horizon of the trace before, read back in a pass after the file
    /// opened.
    #[inline(never)]
    fn horizon(&mut self, bulk: &mut impl ReadWrite) {
        let Some(mut window) = psram::owned(tail::WINDOW) else {
            return;
        };
        if let Some(horizon) = self.file.horizon(bulk, window.as_mut(), &mut self.sector) {
            self.log.borrow_mut().record(&horizon.event());
        }
    }

    fn pass(&mut self, bulk: &mut impl ReadWrite, card: Card, now: u64) {
        let mut log = self.log.borrow_mut();
        let Some(recorder) = &mut log.recorder else {
            return;
        };
        let opening = !self.file.is_open();
        let start = &mut self.start;
        let header = || {
            let start = start.take()?;
            let boot = start.boot;
            Some((start.header()?, boot))
        };
        let passed = self
            .file
            .pass(recorder, bulk, card, header, now, &mut self.sector);
        if let File::Open(_, _, drain) = &self.file
            && opening
        {
            log::info!(target: "trace", "recording file={}", drain.name().as_str());
        }
        Self::ended(&mut log, passed);
    }

    /// Append everything the spool holds and sync: before a host takes the
    /// card, a restart, or a power transition.
    pub fn sync(&mut self, bulk: &mut impl ReadWrite, now: u64) {
        let mut log = self.log.borrow_mut();
        let Some(recorder) = &mut log.recorder else {
            return;
        };
        if device::capabilities::power::ACCESS.closed() {
            return;
        }
        let synced = self.file.sync(recorder, bulk, now, &mut self.sector);
        Self::ended(&mut log, synced);
    }

    /// Before a transfer session; see [`File::before_host`].
    pub fn before_host(&mut self, bulk: &mut impl ReadWrite, now: u64) {
        let mut log = self.log.borrow_mut();
        let Some(recorder) = &mut log.recorder else {
            return;
        };
        let before = self.file.before_host(recorder, bulk, now, &mut self.sector);
        Self::ended(&mut log, before);
    }

    /// After a transfer session (`unseen`) or a recorded volume command,
    /// which may have changed the card; see [`File::after_host`]. True
    /// when the log forgot the card: the caller reads the volume table
    /// again.
    pub fn after_host(&mut self, bulk: &mut impl ReadWrite, unseen: bool, now: u64) -> bool {
        let mut log = self.log.borrow_mut();
        if log.recorder.is_none() {
            return false;
        }
        let after = self
            .file
            .after_host(&mut log, bulk, unseen, now, &mut self.sector);
        Self::ended(&mut log, after.map(drop));
        after == Ok(true)
    }

    /// A diagnostic wrote the card unseen: nothing of it is determined.
    pub fn forget_card(&self) {
        self.log
            .borrow_mut()
            .forget(firmware_services::trace::Medium::Bulk);
    }

    pub fn storage_formatted(&mut self) {
        self.file.formatted();
        Self::off(&mut self.log.borrow_mut());
    }

    fn ended(log: &mut WorldLog, result: Result<(), &'static str>) {
        if let Err(why) = result {
            log::warn!(target: "trace", "ended: {why}");
            Self::off(log);
        }
    }

    /// Stop recording and give the spool's PSRAM back: a crash then leaves
    /// nothing to salvage.
    fn off(log: &mut WorldLog) {
        log.off();
        psram::forget();
    }

    pub fn status(&self, out: &mut dyn Write) {
        let log = self.log.borrow();
        self.file
            .status(log.recorder.as_ref().map(|recorder| recorder.sink()), out);
    }
}

impl Start {
    // Runs once per boot; kept out of line so its buffers stay off the
    // console task's frame.
    #[inline(never)]
    fn header(self) -> Option<&'static [u8]> {
        let out: &'static mut [u8] = self.header;
        let build = core::hint::black_box(option_env!("CYCLING_BUILD_HASH").unwrap_or(""));
        let stamp = origin::Stamp {
            build,
            device: device::fitted::NAME,
            boot: self.boot,
        };
        let len = origin::header(stamp, self.terrain, out)?;
        let out: &'static [u8] = out;
        Some(&out[..len])
    }
}

fn remains(kept: &psram::Kept) -> firmware_services::trace::tail::Remains<'_> {
    let (ring, open) = kept.ring_and_open();
    firmware_services::trace::tail::Remains { ring, open }
}
