//! A line of a static page's words, which the runtime prepared for the
//! frame: a setup step's title and text, the phone's lines, Ready's
//! headline. Each line has its faces, largest first, and the longest text
//! it shows, so accept refuses a box too small for it.
use crate::compose::data::{Data, PageData};
use crate::compose::input::{Action, OnPage};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{AMBER, Align, RED, SECONDARY, WHITE};
use crate::screens::phone::{
    GROUP, NAME, NOTE, PASSWORD_ALPHABET, PASSWORD_GROUPS, PROBLEM, SECRET, SSID_ALPHABET,
    SSID_PREFIX, STEP, TITLE_LINE,
};
use crate::screens::scaled::Scaled;
use crate::screens::setup::STEPS;
use firmware_shell::rendering::font::{Face, FaceRole, SPILL};

pub struct Words;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Says {
    SetupTitle,
    SetupFirst,
    SetupSecond,
    PhoneStep,
    PhoneTitle,
    PhoneName,
    PhoneSecret,
    PhoneNote,
    ReadyHeadline,
}

impl Says {
    const ALL: [Self; 9] = [
        Self::SetupTitle,
        Self::SetupFirst,
        Self::SetupSecond,
        Self::PhoneStep,
        Self::PhoneTitle,
        Self::PhoneName,
        Self::PhoneSecret,
        Self::PhoneNote,
        Self::ReadyHeadline,
    ];

    const fn faces(self) -> &'static [FaceRole] {
        match self {
            Self::SetupTitle => &[FaceRole::Title],
            Self::PhoneTitle => &[FaceRole::Title, FaceRole::Body, FaceRole::Caption],
            Self::ReadyHeadline => &[FaceRole::Title, FaceRole::Body],
            Self::PhoneName | Self::PhoneSecret => &[FaceRole::Body, FaceRole::Caption],
            Self::SetupFirst | Self::SetupSecond | Self::PhoneStep | Self::PhoneNote => {
                &[FaceRole::Caption]
            }
        }
    }

    const fn align(self) -> Align {
        match self {
            Self::SetupTitle | Self::SetupFirst | Self::SetupSecond | Self::ReadyHeadline => {
                Align::Left
            }
            _ => Align::Center,
        }
    }

    /// The longest texts the line shows: what its box must hold.
    fn longest(self) -> heapless::Vec<&'static str, 6> {
        let fixed = |text: &'static str| [text].into_iter().collect();
        let many = |texts: &[&'static str]| texts.iter().copied().collect();
        match self {
            Self::SetupTitle => STEPS.iter().map(|step| step.title()).collect(),
            Self::SetupFirst => STEPS.iter().map(|step| step.text()[0]).collect(),
            Self::SetupSecond => STEPS.iter().map(|step| step.text()[1]).collect(),
            Self::PhoneStep => many(&[
                "PHONE DISCONNECTED",
                "CONNECT PHONE",
                "STEP 1 OF 2",
                "STEP 2 OF 2",
                "ON HOME WI-FI",
            ]),
            Self::PhoneTitle => many(&[
                "ENDED AFTER 10 MIN IDLE",
                "WI-FI DID NOT START",
                "NOT DURING A RIDE",
                "SESSION ENDED",
                "STARTING WI-FI",
                "OPEN YOUR RIDES",
            ]),
            Self::PhoneName => many(&["WAITING FOR WI-FI", "255.255.255.255"]),
            Self::PhoneSecret => heapless::Vec::new(),
            // Clients joined are a u8 and files sent a u32.
            Self::PhoneNote => many(&[
                "ENDS AFTER 10 MIN IDLE",
                "SCAN WITH THE CAMERA",
                "TAP TO TRY AGAIN",
                "255 JOINED",
                "4294967295 SENT",
            ]),
            Self::ReadyHeadline => fixed("ALMOST READY"),
        }
    }

    /// A line the device makes up from an alphabet: its prefix, the
    /// alphabet, and how many groups of `GROUP` characters follow.
    const fn made(self) -> Option<(&'static str, &'static [u8], usize)> {
        match self {
            Self::PhoneName => Some((SSID_PREFIX, SSID_ALPHABET, 1)),
            Self::PhoneSecret => Some(("", PASSWORD_ALPHABET, PASSWORD_GROUPS)),
            _ => None,
        }
    }

    /// The columns its widest text takes in `face`: the longest it shows,
    /// or a made-up line with its alphabet's widest glyph everywhere.
    fn widest(self, face: &Face) -> u16 {
        let fixed = self
            .longest()
            .into_iter()
            .map(|text| Form::text(face, text.as_bytes()).width);
        let made = self.made().map(|(prefix, alphabet, groups)| {
            let glyph = alphabet.iter().map(|&byte| face.glyph(byte).advance).max();
            let characters = groups * GROUP * usize::from(glyph.unwrap_or_default());
            let spaces = (groups - 1) * usize::from(face.glyph(b' ').advance);
            (face.width(prefix.as_bytes()) + characters + spaces + SPILL) as u16
        });
        fixed.chain(made).max().unwrap_or_default()
    }

    /// The rows its glyphs reach under the baseline in any of its faces,
    /// such as a comma's tail.
    fn descent(self, at: &Scaled) -> usize {
        let below = |role: FaceRole, text: &str| {
            let face = at.face(role);
            let baseline = usize::from(face.top) + usize::from(face.cap);
            text.bytes()
                .map(|byte| face.glyph(byte))
                .map(|glyph| (usize::from(glyph.y) + usize::from(glyph.h)).saturating_sub(baseline))
                .max()
                .unwrap_or_default()
        };
        let longest = self.longest();
        let alphabet = self.made().map_or("", |(_, alphabet, _)| {
            core::str::from_utf8(alphabet).unwrap_or_default()
        });
        self.faces()
            .iter()
            .flat_map(|&role| {
                let texts = longest.iter().copied().chain([alphabet]);
                texts.map(move |text| below(role, text))
            })
            .max()
            .unwrap_or_default()
    }

    /// The text this frame shows, and its colour.
    fn says(self, data: &dyn Data) -> (&[u8], u16) {
        let none = (&b""[..], WHITE);
        match (self, data.page()) {
            (Self::SetupTitle, PageData::Setup { step, .. }) => (step.title().as_bytes(), WHITE),
            (Self::SetupFirst, PageData::Setup { step, .. }) => {
                (step.text()[0].as_bytes(), SECONDARY)
            }
            (Self::SetupSecond, PageData::Setup { step, .. }) => {
                (step.text()[1].as_bytes(), SECONDARY)
            }
            (_, PageData::Phone { lines, .. }) => {
                let line = |at: usize| lines.get(at).map_or(&b""[..], |line| line.as_bytes());
                let problem = !line(PROBLEM).is_empty();
                match self {
                    Self::PhoneStep => (line(STEP), SECONDARY),
                    Self::PhoneTitle => (line(TITLE_LINE), if problem { AMBER } else { WHITE }),
                    Self::PhoneName => (line(NAME), WHITE),
                    Self::PhoneSecret => (line(SECRET), WHITE),
                    Self::PhoneNote => (line(NOTE), SECONDARY),
                    _ => none,
                }
            }
            (Self::ReadyHeadline, PageData::Ready(readiness)) => {
                if !readiness.can_start() {
                    (b"NOT READY", RED)
                } else if readiness.all_set() {
                    (b"READY", WHITE)
                } else {
                    (b"ALMOST READY", WHITE)
                }
            }
            _ => none,
        }
    }
}

impl Arg for Says {
    fn decode(raw: u8) -> Option<Self> {
        Self::ALL.get(usize::from(raw)).copied()
    }

    fn encode(self) -> u8 {
        self as u8
    }
}

impl Tile for Words {
    type Arg = Says;
    const NAME: &'static str = "says";

    /// One look a face, narrowest last; each keeps the largest face's
    /// rows and the tails under them, so the line holds its place
    /// whichever face its text takes.
    fn forms(says: Says, at: &Scaled) -> Forms {
        let height = (at.face(says.faces()[0]).fit_height() + says.descent(at)) as u16;
        says.faces()
            .iter()
            .map(|&role| Form {
                width: says.widest(at.face(role)),
                height,
                shapes: 0,
                runs: 1,
            })
            .collect()
    }

    /// The text in the largest face its box fits.
    fn prepare(says: Says, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let (text, color) = says.says(data);
        if text.is_empty() {
            return;
        }
        let faces: heapless::Vec<_, 3> = says.faces().iter().map(|&role| at.face(role)).collect();
        let rect = placed.rect.rect();
        let align = says.align();
        let x = match align {
            Align::Left => rect.left,
            Align::Center => rect.center_x(),
            Align::Right => rect.right,
        };
        let base = rect.bottom - says.descent(at);
        out.fit(&faces, text, rect.width(), x, base, align, color);
    }

    /// The phone's note says TAP TO TRY AGAIN once a session ended, and a
    /// tap on it then tries again.
    fn action(says: Says) -> Option<Action> {
        (says == Says::PhoneNote).then_some(Action::Page(OnPage::PhoneRetry))
    }
}
