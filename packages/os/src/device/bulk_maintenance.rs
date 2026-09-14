//! Explicitly selected maintenance image. Raw MMC writes never exist in normal
//! images. A new session disarms writes; every write requires its token, an armed
//! relative range, a complete CRC-checked sector and verified device completion.
use cycling_os::{bulk::Read, capabilities::Console};
use embassy_time::{Duration, Instant, Timer};

const MAX_SECTORS: usize = 7;
const TIMEOUT: Duration = Duration::from_secs(10);

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut v = i as u32;
        let mut n = 0;
        while n < 8 {
            v = (v >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(v & 1));
            n += 1;
        }
        table[i] = v;
        i += 1;
    }
    table
}
const CRC_TABLE: [u32; 256] = crc_table();
fn crc(data: &[u8]) -> u32 {
    let mut v = u32::MAX;
    for byte in data {
        v = (v >> 8) ^ CRC_TABLE[((v as u8) ^ byte) as usize];
    }
    !v
}
fn word(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}
fn range(start: u32, count: u32, total: u64) -> bool {
    count != 0 && u64::from(start) + u64::from(count) <= total
}
async fn receive(usb: &mut super::usb::Usb, bytes: &mut [u8]) -> Result<(), ()> {
    let deadline = Instant::now() + TIMEOUT;
    for byte in bytes {
        loop {
            if let Some(value) = usb.read() {
                *byte = value;
                break;
            }
            if Instant::now() >= deadline {
                return Err(());
            }
            Timer::after_micros(100).await;
        }
    }
    Ok(())
}
async fn transmit(usb: &mut super::usb::Usb, bytes: &[u8]) -> Result<(), ()> {
    let deadline = Instant::now() + TIMEOUT;
    for packet in bytes.chunks(64) {
        for byte in packet {
            while !usb.write(*byte) {
                if Instant::now() >= deadline {
                    return Err(());
                }
                Timer::after_micros(100).await;
            }
        }
        usb.flush();
        while !usb.tx_ready() {
            if Instant::now() >= deadline {
                return Err(());
            }
            Timer::after_micros(100).await;
        }
    }
    Ok(())
}
async fn reply(usb: &mut super::usb::Usb, fields: [u32; 6], payload: &[u8]) -> Result<(), ()> {
    let mut header = [0u8; 32];
    header[..4].copy_from_slice(b"C6RP");
    for (i, value) in fields.iter().enumerate() {
        header[4 + i * 4..8 + i * 4].copy_from_slice(&value.to_le_bytes());
    }
    let checksum = crc(&header[..28]);
    header[28..].copy_from_slice(&checksum.to_le_bytes());
    transmit(usb, &header).await?;
    transmit(usb, payload).await
}

#[embassy_executor::task]
pub async fn run(mut usb: super::usb::Usb, bulk: super::c606::Bulk) {
    let mut media = bulk.maintenance_reader();
    let mut token = 0;
    let mut armed = None::<(u32, u32)>;
    let mut last_id = 0;
    let mut header = [0u8; 32];
    let mut buffer = [0u8; MAX_SECTORS * 512];
    loop {
        // Search for framing magic, without interpreting boot text or a partial
        // prior packet as a command. Any malformed request disarms writes.
        let mut found = 0;
        while found < 4 {
            let Some(byte) = usb.read() else {
                Timer::after_micros(100).await;
                continue;
            };
            if byte == b"C6RQ"[found] {
                header[found] = byte;
                found += 1;
            } else {
                found = usize::from(byte == b'C');
                header[0] = byte;
                armed = None;
            }
        }
        if receive(&mut usb, &mut header[4..]).await.is_err()
            || crc(&header[..28]) != word(&header, 28)
        {
            armed = None;
            continue;
        }
        let op = word(&header, 4);
        let id = word(&header, 8);
        let start = word(&header, 12);
        let count = word(&header, 16);
        let supplied_token = word(&header, 20);
        let data_crc = word(&header, 24);
        if op != 1 && op != 5 && (id <= last_id || token == 0) {
            armed = None;
            let _ = reply(&mut usb, [id, 1, start, 0, 3, 0], &[]).await;
            continue;
        }
        last_id = id;
        let Some(media) = media.as_mut() else {
            let _ = reply(&mut usb, [id, 2, start, 0, 3, 0], &[]).await;
            continue;
        };
        if op == 1 {
            armed = None;
            token = esp_hal::rng::Rng::new().random().max(1);
            let Ok(info) = media.info() else {
                let _ = reply(&mut usb, [id, 2, start, 0, 3, 0], &[]).await;
                continue;
            };
            let mut info_bytes = [0u8; 16];
            info_bytes[..8].copy_from_slice(&info.sectors.to_le_bytes());
            info_bytes[8..12].copy_from_slice(&512u32.to_le_bytes());
            info_bytes[12..16].copy_from_slice(&token.to_le_bytes());
            let _ = reply(&mut usb, [id, 0, 0, 0, 2, crc(&info_bytes)], &info_bytes).await;
            continue;
        }
        if op == 5 {
            armed = None;
            let result = match super::services::power::ACCESS.enter() {
                Ok(_access) => media.recover(),
                Err(_) => Err(cycling_os::bulk::Error::Unavailable),
            };
            let _ = reply(&mut usb, [id, u32::from(result.is_err()), 0, 0, 3, 0], &[]).await;
            continue;
        }
        if op == 6 {
            armed = None;
            let result = if start != 0 || count != 0 {
                Err(cycling_os::bulk::Error::Range)
            } else {
                match super::services::power::ACCESS.enter() {
                    Ok(_access) => media.wide_read_mode(),
                    Err(_) => Err(cycling_os::bulk::Error::Unavailable),
                }
            };
            let _ = reply(
                &mut usb,
                [id, if result.is_ok() { 0 } else { 7 }, 0, 0, 3, 0],
                &[],
            )
            .await;
            continue;
        }
        let valid = media
            .info()
            .is_ok_and(|info| range(start, count, info.sectors));
        if !valid {
            armed = None;
            let _ = reply(&mut usb, [id, 3, start, 0, 3, 0], &[]).await;
            continue;
        }
        if op == 3 && supplied_token == token {
            armed = Some((start, count));
            if reply(&mut usb, [id, 0, start, count, 3, 0], &[])
                .await
                .is_err()
            {
                armed = None;
            }
        } else if op == 2 {
            // Clock changes do not alter persistent card configuration. The
            // one-bit 20MHz path has existing hardware read evidence.
            let clock_result = match super::services::power::ACCESS.enter() {
                Ok(_access) => media.clock(20_000_000),
                Err(_) => Err(cycling_os::bulk::Error::Unavailable),
            };
            if clock_result.is_err() {
                armed = None;
                let _ = reply(&mut usb, [id, 4, start, 0, 3, 0], &[]).await;
                continue;
            }
            let mut sector = start;
            let mut remaining = count;
            while remaining != 0 {
                let n = remaining.min(MAX_SECTORS as u32);
                let bytes = &mut buffer[..n as usize * 512];
                let result = match super::services::power::ACCESS.enter() {
                    Ok(_access) => media.read_blocks(u64::from(sector), bytes),
                    Err(_) => Err(cycling_os::bulk::Error::Unavailable),
                };
                if result.is_err() {
                    armed = None;
                    let _ = reply(&mut usb, [id, 4, sector, 0, 3, 0], &[]).await;
                    break;
                }
                let checksum = crc(bytes);
                let uniform = bytes.iter().all(|v| *v == bytes[0]);
                let payload = if uniform { &bytes[..1] } else { bytes };
                if reply(
                    &mut usb,
                    [id, 0, sector, n, u32::from(uniform), checksum],
                    payload,
                )
                .await
                .is_err()
                {
                    armed = None;
                    break;
                }
                sector += n;
                remaining -= n;
                // Give peripheral owners a chance between bounded transfers.
                embassy_futures::yield_now().await;
            }
        } else if op == 4
            && count == 1
            && supplied_token == token
            && armed
                .is_some_and(|(a, n)| start >= a && u64::from(start) < u64::from(a) + u64::from(n))
        {
            if receive(&mut usb, &mut buffer[..512]).await.is_err()
                || crc(&buffer[..512]) != data_crc
            {
                armed = None;
                let _ = reply(&mut usb, [id, 5, start, 0, 3, 0], &[]).await;
                continue;
            }
            let result = match super::services::power::ACCESS.enter() {
                Ok(_access) => {
                    media.write_sector_verified(u64::from(start), buffer[..512].try_into().unwrap())
                }
                Err(_) => Err(cycling_os::bulk::Error::Unavailable),
            };
            if result.is_err() {
                armed = None;
            }
            if reply(
                &mut usb,
                [
                    id,
                    if result.is_ok() { 0 } else { 6 },
                    start,
                    if result.is_ok() { 1 } else { 0 },
                    3,
                    if result.is_ok() { data_crc } else { 0 },
                ],
                &[],
            )
            .await
            .is_err()
            {
                armed = None;
            }
        } else {
            armed = None;
            let _ = reply(&mut usb, [id, 1, start, 0, 3, 0], &[]).await;
        }
    }
}
