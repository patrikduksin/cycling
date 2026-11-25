//! The device's home Wi-Fi over the phone session: what the station sees,
//! a scan, joining a network and forgetting it. The routes sit behind the
//! session token like every `/api` route, and they hold the same station
//! and save through the same `Shell::save_wifi` as the terminal's `WIFI`
//! commands, so the two cannot disagree on what is saved.
//!
//! `GET /api/wifi` answers
//!
//! ```json
//! {"configured":"home"|null,"state":"connected","online":true,"rssi":-58|null,
//!  "op":{"seq":7,"pending":false,"error":"wrong_password"|null},
//!  "networks":[{"ssid":"home","rssi":-58,"secure":true}]}
//! ```
//!
//! - `configured`: the saved network's SSID.
//! - `state`: `off` (nothing saved), `idle` (saved, the station not
//!   joining), `joining`, `connected` (an address, the connectivity check
//!   passed or pending), `no_internet` (associated, the check failed and the
//!   station is recovering) or `failed`, when `op.error` says why.
//! - `rssi`: the associated network's signal in dBm.
//! - `op`: the station's last operation, by the `seq` a write answered with.
//!   A join stays `pending` until its connect attempt ends. `error` is a
//!   [`ControlError`] word; a wrong password reads `wrong_password` (the
//!   access point refused the handshake) and an SSID nobody answers
//!   `network_not_found`.
//! - `networks`: the last scan, one entry per SSID at its strongest signal,
//!   strongest first; `secure` is false for an open network, which the
//!   station cannot join. Hidden networks are left out.
//!
//! `POST /api/wifi/scan` starts a scan and `DELETE /api/wifi` forgets the
//! saved network; `PUT /api/wifi` with `{"ssid":"…","password":"…"}` saves
//! it and joins. Each answers `202 {"seq":n}`, `409 {"error":"busy"}` while
//! an operation is pending, and a join `400` with `json`, `ssid` or
//! `password` when the body is not one the station can take.

use super::{HEAD_GAP, Job, Maybe, Runtime, TEXT, frame};
use core::fmt::Write;
use device_api::connectivity::{
    ControlError, NetworkDiscovery, Operation, Text, Wifi, WifiConfig, WifiOperation, WifiState,
};
use device_api::server::{Method, Request, Server};
use firmware_shell::shell::Shell;

/// The longest join body taken: an SSID and password with every byte
/// escaped.
const BODY_MAX: u32 = 768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Asked {
    Read,
    Scan,
    /// `received` of the body's bytes so far, gathered in `Work::text`.
    Join {
        received: u32,
    },
    Forget,
}

/// A network the phone asked to join: the station takes its configuration,
/// then connects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Join {
    #[default]
    Idle,
    /// The station's `Configure`, by its sequence.
    Configuring { sequence: u32 },
}

impl Runtime {
    pub(super) fn route_wifi(&mut self, request: &Request) {
        let id = request.id;
        let asked = match (request.method, request.path.as_str()) {
            (Method::Get, "/api/wifi") => Asked::Read,
            (Method::Post, "/api/wifi/scan") => Asked::Scan,
            (Method::Put, "/api/wifi") if request.body == 0 => {
                return self.reply(id, 400, Some(r#"{"error":"json"}"#));
            }
            (Method::Put, "/api/wifi") if request.body > BODY_MAX => {
                return self.reply(id, 413, Some(r#"{"error":"too_large"}"#));
            }
            (Method::Put, "/api/wifi") => {
                self.phone.w().text.clear();
                Asked::Join { received: 0 }
            }
            (Method::Delete, "/api/wifi") => Asked::Forget,
            (_, "/api/wifi" | "/api/wifi/scan") => return self.reply(id, 405, None),
            _ => return self.reply(id, 404, Some(r#"{"error":"not_found"}"#)),
        };
        self.phone.w().job = Job::Wifi { id, asked };
    }

    /// Carry a join on to its connect, and answer a Wi-Fi request the
    /// session routed here.
    pub fn serve_wifi<D, I, P, B>(
        &mut self,
        server: &mut impl Server,
        wifi: &mut impl Wifi,
        shell: &mut Shell<D, I, P, B>,
        now: u64,
    ) where
        D: device_api::display::Display,
        I: device_api::input::InputSource,
        P: device_api::power::Power,
        B: device_api::storage::OwnedFlash,
    {
        self.advance_join(wifi);
        let Some(Job::Wifi { id, asked }) = self.phone.work.as_deref().map(|work| &work.job) else {
            return;
        };
        let (id, asked) = (*id, *asked);
        match asked {
            Asked::Read => self.wifi_reply(id, wifi, shell.settings.wifi),
            Asked::Scan if self.wifi_busy(wifi) => self.reply(id, 409, Some(r#"{"error":"busy"}"#)),
            Asked::Scan => match wifi.request(WifiOperation::Scan) {
                Ok(()) => self.wifi_accepted(id, wifi.control().sequence),
                Err(_) => self.reply(id, 503, Some(r#"{"error":"radio"}"#)),
            },
            Asked::Join { received } => {
                let Some(body) = server.request().filter(|r| r.id == id).map(|r| r.body) else {
                    return;
                };
                let received = self.take_body(server, id, received, body, now);
                if received < body {
                    self.phone.w().job = Job::Wifi {
                        id,
                        asked: Asked::Join { received },
                    };
                    return;
                }
                match join_config(&self.phone.w().text) {
                    Ok(config) => self.change(id, wifi, shell, Some(config)),
                    Err(field) => {
                        let mut json = heapless::String::<32>::new();
                        let _ = write!(json, r#"{{"error":"{field}"}}"#);
                        self.reply(id, 400, Some(&json));
                    }
                }
            }
            Asked::Forget => self.change(id, wifi, shell, None),
        }
    }

    fn wifi_busy(&self, wifi: &impl Wifi) -> bool {
        wifi.control().operation == Operation::Pending || self.phone.join != Join::Idle
    }

    /// Save `config` as the shell saves the terminal's, then hand it to the
    /// station; a join connects once the station took it.
    fn change<D, I, P, B>(
        &mut self,
        id: u32,
        wifi: &mut impl Wifi,
        shell: &mut Shell<D, I, P, B>,
        config: Option<WifiConfig>,
    ) where
        D: device_api::display::Display,
        I: device_api::input::InputSource,
        P: device_api::power::Power,
        B: device_api::storage::OwnedFlash,
    {
        if self.wifi_busy(wifi) {
            return self.reply(id, 409, Some(r#"{"error":"busy"}"#));
        }
        if shell.save_wifi(wifi.control().operation, config).is_err() {
            return self.reply(id, 503, Some(r#"{"error":"storage"}"#));
        }
        if wifi.request(WifiOperation::Configure(config)).is_err() {
            return self.reply(id, 503, Some(r#"{"error":"radio","saved":true}"#));
        }
        let sequence = wifi.control().sequence;
        log::info!(target: "phone", "VANA_PHONE wifi saved={} seq={sequence}", config.is_some());
        if config.is_some() {
            self.phone.join = Join::Configuring { sequence };
        }
        self.wifi_accepted(id, sequence);
    }

    /// Connect once the join's configuration completed; drop the join when
    /// it failed or another operation took the station.
    fn advance_join(&mut self, wifi: &mut impl Wifi) {
        let Join::Configuring { sequence } = self.phone.join else {
            return;
        };
        let control = wifi.control();
        if control.sequence == sequence && control.operation == Operation::Pending {
            return;
        }
        if control.sequence == sequence && control.operation == Operation::Completed {
            let _ = wifi.request(WifiOperation::Connect);
        }
        self.phone.join = Join::Idle;
    }

    fn wifi_accepted(&mut self, id: u32, sequence: u32) {
        let mut json = heapless::String::<24>::new();
        let _ = write!(json, r#"{{"seq":{sequence}}}"#);
        self.reply(id, 202, Some(&json));
    }

    fn wifi_reply(&mut self, id: u32, wifi: &impl Wifi, saved: Option<WifiConfig>) {
        let control = wifi.control();
        let joining = self.phone.join != Join::Idle;
        let online = wifi.online();
        let state = state_word(saved.is_some(), joining, wifi.state(), online);
        let error = match control.operation {
            Operation::Failed => control.error.map(ControlError::name),
            _ => None,
        };
        let networks = strongest(wifi.discoveries());
        let rssi = wifi.rssi();
        let work = self.phone.w();
        let text = &mut work.text;
        text.clear();
        let _ = text.resize(HEAD_GAP, 0);
        let _ = text.extend_from_slice(br#"{"configured":"#);
        match saved {
            Some(config) => {
                let _ = string(text, config.ssid.text());
            }
            None => {
                let _ = text.extend_from_slice(b"null");
            }
        }
        let _ = write!(
            text,
            r#","state":"{state}","online":{online},"rssi":{},"op":{{"seq":{},"pending":{},"error":"#,
            Maybe(rssi),
            control.sequence,
            control.operation == Operation::Pending || joining,
        );
        let _ = match error {
            Some(word) => write!(text, r#""{word}""#),
            None => write!(text, "null"),
        };
        let _ = text.extend_from_slice(br#"},"networks":["#);
        for (index, network) in networks.iter().flatten().enumerate() {
            let mark = text.len();
            if entry(text, index > 0, network).is_err() || text.len() + 2 > TEXT {
                // The strongest networks are already in; leave out the rest.
                text.truncate(mark);
                break;
            }
        }
        let _ = text.extend_from_slice(b"]}");
        frame(text, 200);
        work.job = Job::Text { id, at: 0 };
    }
}

fn state_word(saved: bool, joining: bool, state: WifiState, online: bool) -> &'static str {
    match state {
        _ if joining => "joining",
        WifiState::Unconfigured if saved => "idle",
        WifiState::Unconfigured => "off",
        WifiState::Connecting | WifiState::Associated => "joining",
        WifiState::AddressReady | WifiState::Verified => "connected",
        WifiState::Retrying if online => "no_internet",
        WifiState::Retrying => "joining",
        WifiState::Failed => "failed",
    }
}

/// One entry per SSID at its strongest, strongest first.
fn strongest(found: [Option<NetworkDiscovery>; 8]) -> [Option<NetworkDiscovery>; 8] {
    let mut kept = [None; 8];
    let mut count = 0;
    for network in found.into_iter().flatten() {
        if network.ssid.is_empty() {
            continue;
        }
        match kept[..count]
            .iter_mut()
            .flatten()
            .find(|seen: &&mut NetworkDiscovery| seen.ssid == network.ssid)
        {
            Some(seen) if network.rssi > seen.rssi => *seen = network,
            Some(_) => {}
            None => {
                kept[count] = Some(network);
                count += 1;
            }
        }
    }
    kept[..count].sort_unstable_by_key(|network| network.map(|n| core::cmp::Reverse(n.rssi)));
    kept
}

/// One `networks` entry, after a comma unless it is the first.
fn entry(
    text: &mut heapless::Vec<u8, TEXT>,
    comma: bool,
    network: &NetworkDiscovery,
) -> Result<(), ()> {
    if comma {
        text.push(b',').map_err(drop)?;
    }
    text.extend_from_slice(br#"{"ssid":"#).map_err(drop)?;
    string(text, network.ssid.text())?;
    write!(
        text,
        r#","rssi":{},"secure":{}}}"#,
        network.rssi, network.secure
    )
    .map_err(drop)
}

/// `value` as a JSON string.
fn string(text: &mut heapless::Vec<u8, TEXT>, value: &str) -> Result<(), ()> {
    text.push(b'"').map_err(drop)?;
    for byte in value.bytes() {
        match byte {
            b'"' | b'\\' => text.extend_from_slice(&[b'\\', byte]).map_err(drop)?,
            0..0x20 => write!(text, "\\u{byte:04x}").map_err(drop)?,
            _ => text.push(byte).map_err(drop)?,
        }
    }
    text.push(b'"').map_err(drop)
}

/// The network a join body names, or the word for what is wrong with it.
fn join_config(body: &[u8]) -> Result<WifiConfig, &'static str> {
    let mut ssid = Field::new();
    let mut password = Field::new();
    let mut json = Cursor { body, at: 0 };
    json.object(|key, value| match key {
        b"ssid" => ssid = value,
        b"password" => password = value,
        _ => {}
    })
    .ok_or("json")?;
    let ssid = ssid
        .whole()
        .filter(|ssid| !ssid.is_empty() && Text::<32>::new(ssid).is_some())
        .ok_or("ssid")?;
    let password = password.whole().ok_or("password")?;
    WifiConfig::new(ssid, password, false).ok_or("password")
}

/// A decoded string value, `long` when it did not fit.
#[derive(Clone)]
struct Field {
    bytes: heapless::Vec<u8, 64>,
    long: bool,
}

impl Field {
    fn new() -> Self {
        Self {
            bytes: heapless::Vec::new(),
            long: false,
        }
    }

    fn whole(&self) -> Option<&[u8]> {
        (!self.long).then_some(&self.bytes)
    }

    fn put(&mut self, bytes: &[u8]) {
        self.long |= self.bytes.extend_from_slice(bytes).is_err();
    }
}

/// A JSON object of string values, the only shape a join takes.
struct Cursor<'a> {
    body: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn peek(&mut self) -> Option<u8> {
        while let Some(&byte) = self.body.get(self.at) {
            if !matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
                return Some(byte);
            }
            self.at += 1;
        }
        None
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.at += 1)
    }

    fn object(&mut self, mut each: impl FnMut(&[u8], Field)) -> Option<()> {
        self.eat(b'{')?;
        if self.eat(b'}').is_some() {
            return self.end();
        }
        loop {
            let key = self.string()?;
            self.eat(b':')?;
            let value = self.string()?;
            each(&key.bytes, value);
            if self.eat(b'}').is_some() {
                return self.end();
            }
            self.eat(b',')?;
        }
    }

    fn end(&mut self) -> Option<()> {
        self.peek().is_none().then_some(())
    }

    fn string(&mut self) -> Option<Field> {
        self.eat(b'"')?;
        let mut field = Field::new();
        loop {
            let byte = *self.body.get(self.at)?;
            self.at += 1;
            match byte {
                b'"' => return Some(field),
                0..0x20 => return None,
                b'\\' => {
                    let escape = *self.body.get(self.at)?;
                    self.at += 1;
                    let decoded = match escape {
                        b'"' | b'\\' | b'/' => escape,
                        b'b' => 0x08,
                        b'f' => 0x0c,
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'u' => {
                            let ch = self.unicode()?;
                            field.put(ch.encode_utf8(&mut [0; 4]).as_bytes());
                            continue;
                        }
                        _ => return None,
                    };
                    field.put(&[decoded]);
                }
                _ => field.put(&[byte]),
            }
        }
    }

    /// The character after `\u`, joining a surrogate pair.
    fn unicode(&mut self) -> Option<char> {
        let high = self.hex4()?;
        if !(0xd800..0xdc00).contains(&high) {
            return char::from_u32(high);
        }
        if self.body.get(self.at..self.at + 2)? != b"\\u" {
            return None;
        }
        self.at += 2;
        let low = self.hex4()?;
        if !(0xdc00..0xe000).contains(&low) {
            return None;
        }
        char::from_u32(0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00))
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = core::str::from_utf8(self.body.get(self.at..self.at + 4)?).ok()?;
        self.at += 4;
        u32::from_str_radix(digits, 16).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(ssid: &str, rssi: i8) -> Option<NetworkDiscovery> {
        Some(NetworkDiscovery {
            ssid: Text::new(ssid.as_bytes()).unwrap(),
            rssi,
            secure: true,
        })
    }

    #[test]
    fn a_join_body_decodes_escapes_and_says_what_is_wrong() {
        // `\u` escapes of an accented letter and of a surrogate pair.
        let body = std::format!(
            r#" {{"ssid":"caf{u}00e9 \"5\"","password":"p\\ss\/word{u}d83d{u}de00"}} "#,
            u = r"\u"
        );
        let config = join_config(body.as_bytes()).unwrap();
        assert_eq!(config.ssid.text(), "café \"5\"");
        assert_eq!(config.password.text(), "p\\ss/word😀");
        for (body, word) in [
            (
                br#"{"ssid":"home","password":"short"}"#.as_slice(),
                "password",
            ),
            (br#"{"ssid":"home"}"#, "password"),
            (br#"{"password":"password"}"#, "ssid"),
            (br#"{"ssid":"","password":"password"}"#, "ssid"),
            (
                br#"{"ssid":"0123456789012345678901234567890123","password":"password"}"#,
                "ssid",
            ),
            (br#"{"ssid":"a\u0000b","password":"password"}"#, "ssid"),
            (br#"{"ssid":"home","password":"password"} x"#, "json"),
            (br#"{"ssid":"home","password":8}"#, "json"),
            (br#"{"ssid":"home" "password":"password"}"#, "json"),
            (br#"{"ssid":"\ud800","password":"password"}"#, "json"),
            (b"not json", "json"),
        ] {
            assert_eq!(
                join_config(body).err(),
                Some(word),
                "{}",
                std::string::String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn networks_keep_each_ssid_at_its_strongest_strongest_first() {
        let kept = strongest([
            network("cafe", -80),
            network("home", -70),
            None,
            network("cafe", -50),
            network("", -20),
            network("home", -75),
            None,
            None,
        ]);
        let seen: std::vec::Vec<_> = kept
            .iter()
            .flatten()
            .map(|n| (n.ssid.text(), n.rssi))
            .collect();
        assert_eq!(seen, [("cafe", -50), ("home", -70)]);
    }

    #[test]
    fn ssids_are_json_strings() {
        let mut text = heapless::Vec::<u8, TEXT>::new();
        string(&mut text, "a\"b\\c\n\u{1}é").unwrap();
        assert_eq!(text.as_slice(), "\"a\\\"b\\\\c\\u000a\\u0001é\"".as_bytes());
    }
}
