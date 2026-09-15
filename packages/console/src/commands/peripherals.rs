//! Ordinary bounded commands over independent read, sound and sensor capabilities.

use core::fmt::Write;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Mmc,
    MmcRead {
        sector: u64,
        offset: u16,
        length: u16,
    },
    MmcClock(u32),
    MmcRecover,
    MmcOwned,
    MmcOwnedRead {
        sector: u64,
        offset: u16,
        length: u16,
    },
    #[cfg(feature = "debug-harness")]
    MmcOwnedTest {
        sector: u64,
        expected_crc: u32,
        fill: u8,
    },
    Sound,
    SoundPatterns,
    SoundPlay(u8),
    SoundStop,
    Gnss,
    GnssPause(u32),
    GnssResume,
    Pressure,
    Motion,
    Companion,
    CompanionQuery,
}
pub fn parse(name: &str, words: &mut core::str::SplitAsciiWhitespace<'_>) -> Option<Command> {
    Some(match name {
        "MMC" => match words.next() {
            None | Some("STATUS") => Command::Mmc,
            Some("READ") => {
                let sector = words.next()?.parse().ok()?;
                let offset = words.next()?.parse().ok()?;
                let length = words.next()?.parse().ok()?;
                firmware_services::bulk::chunk(usize::from(offset), usize::from(length)).ok()?;
                Command::MmcRead {
                    sector,
                    offset,
                    length,
                }
            }
            Some("OWNED") => match words.next() {
                None | Some("STATUS") => Command::MmcOwned,
                Some("READ") => {
                    let sector = words.next()?.parse().ok()?;
                    let offset = words.next()?.parse().ok()?;
                    let length = words.next()?.parse().ok()?;
                    firmware_services::bulk::chunk(usize::from(offset), usize::from(length))
                        .ok()?;
                    Command::MmcOwnedRead {
                        sector,
                        offset,
                        length,
                    }
                }
                #[cfg(feature = "debug-harness")]
                Some("TEST") => Command::MmcOwnedTest {
                    sector: words.next()?.parse().ok()?,
                    expected_crc: u32::from_str_radix(words.next()?, 16).ok()?,
                    fill: u8::from_str_radix(words.next()?, 16).ok()?,
                },
                _ => return None,
            },
            Some("CLOCK") => {
                let hz = words.next()?.parse().ok()?;
                if !matches!(hz, 400_000 | 4_000_000 | 20_000_000) {
                    return None;
                }
                Command::MmcClock(hz)
            }
            Some("RECOVER") => Command::MmcRecover,
            _ => return None,
        },
        "SOUND" => match words.next() {
            None | Some("STATUS") => Command::Sound,
            Some("PATTERNS") => Command::SoundPatterns,
            Some("PLAY") => Command::SoundPlay(words.next()?.parse().ok()?),
            Some("STOP") => Command::SoundStop,
            _ => return None,
        },
        "GNSS" => match words.next() {
            None | Some("STATUS") => Command::Gnss,
            Some("PAUSE") => {
                let ms = words.next()?.parse().ok()?;
                if !(2000..=10000).contains(&ms) {
                    return None;
                }
                Command::GnssPause(ms)
            }
            Some("RESUME") => Command::GnssResume,
            _ => return None,
        },
        "PRESSURE" => Command::Pressure,
        "MOTION" => Command::Motion,
        "COMPANION" => match words.next() {
            None | Some("STATUS") => Command::Companion,
            Some("QUERY") => Command::CompanionQuery,
            _ => return None,
        },
        _ => return None,
    })
}
pub fn execute(
    command: Command,
    now: u64,
    media: &mut (impl device_api::bulk::Read + device_api::bulk::ReadWrite),
    sound: &mut impl device_api::sound::Sound,
    gnss: &mut impl device_api::position_control::Control,
    sensors: &mut impl device_api::sensors::Sensors,
    out: &mut impl Write,
) -> &'static str {
    use crate::protocol::request_status;
    let mut status = "OK";
    match command {
        Command::Mmc => match media.info() {
            Ok(s) => {
                let _ = write!(
                    out,
                    "sectors={} sector_size={} clock_hz={} width={} reads={} failures={} read_us={} access=read-only",
                    s.sectors,
                    s.sector_size,
                    s.clock_hz,
                    s.bus_width,
                    s.reads,
                    s.failures,
                    s.last_read_us
                );
            }
            Err(e) => status = media_error(e),
        },
        Command::MmcRead {
            sector,
            offset,
            length,
        } => {
            let Ok(range) =
                firmware_services::bulk::chunk(usize::from(offset), usize::from(length))
            else {
                return "INVALID";
            };
            let mut data = [0; 512];
            match media.read(sector, &mut data) {
                Ok(()) => {
                    let bytes = &data[range];
                    let crc = firmware_shell::harness::crc32(bytes);
                    let _ = write!(
                        out,
                        "sector={} offset={} length={} crc32={:08x} read_us={} data=",
                        sector,
                        offset,
                        length,
                        crc,
                        media.info().map_or(0, |i| i.last_read_us)
                    );
                    for byte in bytes {
                        let _ = write!(out, "{:02x}", byte);
                    }
                }
                Err(e) => status = media_error(e),
            }
        }
        Command::MmcOwned => match media.owned_info() {
            Ok(info) => {
                let _ = write!(
                    out,
                    "start_sector={} sectors={} sector_size={} access=read-write-relative",
                    info.start_sector, info.sectors, info.sector_size
                );
            }
            Err(e) => status = media_error(e),
        },
        Command::MmcOwnedRead {
            sector,
            offset,
            length,
        } => {
            let Ok(range) =
                firmware_services::bulk::chunk(usize::from(offset), usize::from(length))
            else {
                return "INVALID";
            };
            let mut bytes = [0u8; 512];
            match media.owned_read(sector, &mut bytes) {
                Ok(()) => {
                    let bytes = &bytes[range];
                    let _ = write!(
                        out,
                        "sector={} offset={} length={} crc32={:08x} data=",
                        sector,
                        offset,
                        length,
                        firmware_shell::harness::crc32(bytes)
                    );
                    for byte in bytes {
                        let _ = write!(out, "{:02x}", byte);
                    }
                }
                Err(e) => status = media_error(e),
            }
        }
        #[cfg(feature = "debug-harness")]
        Command::MmcOwnedTest {
            sector,
            expected_crc,
            fill,
        } => {
            let mut bytes = [0u8; 512];
            if let Err(e) = media.owned_read(sector, &mut bytes) {
                return media_error(e);
            }
            if firmware_shell::harness::crc32(&bytes) != expected_crc {
                return "CONFLICT";
            }
            bytes.fill(fill);
            match media.owned_write(sector, &bytes) {
                Ok(()) => {
                    let _ = write!(
                        out,
                        "sector={} length=512 crc32={:08x} verified=true",
                        sector,
                        firmware_shell::harness::crc32(&bytes)
                    );
                }
                Err(e) => status = media_error(e),
            }
        }
        Command::MmcClock(hz) => {
            if let Err(e) = media.clock(hz) {
                status = media_error(e);
            }
        }
        Command::MmcRecover => {
            if let Err(e) = media.recover() {
                status = media_error(e);
            }
        }
        Command::Sound => {
            let s = sound.snapshot();
            let _ = write!(
                out,
                "state={:?} pattern={:?} requested_ms={} submitted_ms={:?} until_ms={} acoustic_ack=false",
                s.state, s.pattern, s.requested_ms, s.submitted_ms, s.until_ms
            );
        }
        Command::SoundPatterns => {
            for p in sound.patterns() {
                let _ = write!(
                    out,
                    "pattern_{}={}Hz,{}ms ",
                    p.id, p.frequency_hz, p.nominal_ms
                );
            }
            let _ = write!(
                out,
                "level=fixed duration=fixed timing=nominal_table_values acoustic_timing=unverified"
            );
        }
        Command::SoundPlay(id) => status = request_status(sound.play(id, now)),
        Command::SoundStop => status = request_status(sound.stop(now)),
        Command::Gnss => {
            let s = gnss.control();
            let _ = write!(
                out,
                "state={:?} at_ms={} resume_at_ms={:?} generation={} control=stream electrical_power=unknown",
                s.state, s.at_ms, s.resume_at_ms, s.generation
            );
        }
        Command::GnssPause(ms) => status = request_status(gnss.pause(ms, now)),
        Command::GnssResume => status = request_status(gnss.resume(now)),
        Command::Pressure => {
            let s = sensors.snapshot(now);
            let _ = write!(
                out,
                "reports={} invalid={} losses={} ",
                s.reports, s.invalid_reports, s.losses
            );
            match s.pressure {
                device_api::observation::Observation::Unavailable => {
                    let _ = write!(out, "state=unavailable");
                }
                device_api::observation::Observation::Fresh { value, received_ms }
                | device_api::observation::Observation::Stale { value, received_ms } => {
                    let fresh = matches!(
                        s.pressure,
                        device_api::observation::Observation::Fresh { .. }
                    );
                    let _ = write!(
                        out,
                        "state={} received_ms={} pressure_centi_pa={} temperature_centi_c={} compensation=companion calibration=unverified",
                        if fresh { "fresh" } else { "stale" },
                        received_ms,
                        value.pressure_centi_pa,
                        value.temperature_centi_c
                    );
                }
            }
        }
        Command::Motion => {
            let s = sensors.snapshot(now);
            let _ = write!(
                out,
                "reports={} invalid={} losses={} units=raw axes=wire-order ",
                s.reports, s.invalid_reports, s.losses
            );
            for (i, o) in s.motion.iter().enumerate() {
                match o {
                    device_api::observation::Observation::Unavailable => {
                        let _ = write!(out, "sample_{}=unavailable ", i + 1);
                    }
                    device_api::observation::Observation::Fresh { value, received_ms }
                    | device_api::observation::Observation::Stale { value, received_ms } => {
                        let _ = write!(
                            out,
                            "sample_{}={},{},{} state_{}={} received_{}={} ",
                            i + 1,
                            value.axes[0],
                            value.axes[1],
                            value.axes[2],
                            i + 1,
                            if matches!(o, device_api::observation::Observation::Fresh { .. }) {
                                "fresh"
                            } else {
                                "stale"
                            },
                            i + 1,
                            received_ms
                        );
                    }
                }
            }
        }
        Command::Companion => {
            let s = sensors.snapshot(now);
            let _ = write!(
                out,
                "query={} identity={:?} fitted_model=unknown installed_version=unverified reports={} invalid={} losses={} startup={} startup_reason={:?}",
                sensors.identity_status(),
                s.identity,
                s.reports,
                s.invalid_reports,
                s.losses,
                sensors.startup_status(),
                sensors.startup_reason()
            );
        }
        Command::CompanionQuery => status = request_status(sensors.query_identity(now)),
    }
    status
}
fn media_error(error: device_api::bulk::Error) -> &'static str {
    match error {
        device_api::bulk::Error::Unsupported => "UNSUPPORTED",
        device_api::bulk::Error::Unavailable => "UNAVAILABLE",
        device_api::bulk::Error::Range => "INVALID",
        device_api::bulk::Error::Failed => "FAILED",
    }
}
