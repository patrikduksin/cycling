//! The rider's screens over the phone session: what each shows, the tiles
//! a screen can hold, a dry run of a changed screen and its save. The
//! device is the only solver: the phone previews from the dry run's rects.
//!
//! A screen travels as its stored nodes, four bytes each, in the body of
//! `POST /api/screens/check?screen=<name>` or `PUT /api/screens/<name>`.
//! A change names the revision the phone read, `?base=`, and is refused
//! with 409 once another change landed or the device read its screens
//! again, and the phone reads them again. A refused screen, or one whose
//! record would not fit, answers 422 with its `Reject` as the reason.

use super::{Job, Runtime, Server, TEXT};
use crate::app::overrides::{Change, NAMED, Named, Saved, named};
use crate::compose::builtin::Choices;
use crate::compose::data::ListKind;
use crate::compose::node::{MAX_DEPTH, MAX_NODES, Node, Size};
use crate::compose::parse::VERSION;
use crate::compose::registry;
use crate::compose::solve::{MAX_LEAVES, Solved};
use crate::compose::{Flags, Reject};
use core::fmt::Write;
use device_api::server::{Method, Request};
use firmware_services::http::query_value;

/// The widest leaf of a dry run: node, form and a rect on the largest panel.
const LEAF: usize = "[31,5,512,512,512,512],".len();
/// Room for one item of a document: a flag combination's every leaf.
const ITEM: usize = 64 + MAX_LEAVES * LEAF;
const _: () = assert!(
    ITEM >= 96 + MAX_NODES * 8,
    "a named screen's nodes in hex fit an item"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Asked {
    Check(&'static Named),
    Save { named: &'static Named, base: u32 },
}

/// A document streamed an item at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Doc {
    /// Each named screen as it shows, and whether the rider changed it.
    Screens,
    /// Each tile with each setting it takes and its looks.
    Catalogue,
    /// Where the proposed screen's tiles go under each flag combination.
    Solves(&'static Named),
}

enum Step {
    Wrote,
    Skip,
    End,
}

impl Runtime {
    pub(super) fn route_screens(&mut self, request: &Request) {
        let id = request.id;
        if self.overrides.is_none() {
            return self.reply(id, 503, Some(r#"{"error":"memory"}"#));
        }
        let base = query_value(&request.query, "base").and_then(|v| v.parse::<u32>().ok());
        match (request.method, request.path.as_str()) {
            (Method::Get, "/api/screens") => self.list(id, Doc::Screens),
            (Method::Get, "/api/screens/catalogue") => self.list(id, Doc::Catalogue),
            (Method::Post, "/api/screens/check") => {
                match query_value(&request.query, "screen").and_then(named) {
                    Some(named) => self.gather_screen(request, Asked::Check(named)),
                    None => self.reply(id, 404, Some(r#"{"error":"screen"}"#)),
                }
            }
            (Method::Delete, "/api/screens") => self.change_screens(id, Change::ResetAll, base),
            (method, path) => {
                let Some(named) = path.strip_prefix("/api/screens/").and_then(named) else {
                    return self.reply(id, 404, Some(r#"{"error":"screen"}"#));
                };
                match (method, base) {
                    (Method::Put, Some(base)) => {
                        self.gather_screen(request, Asked::Save { named, base });
                    }
                    (Method::Delete, _) => {
                        self.change_screens(id, Change::Reset(named.key()), base);
                    }
                    (Method::Put, None) => self.reply(id, 400, Some(r#"{"error":"base"}"#)),
                    _ => self.reply(id, 405, None),
                }
            }
        }
    }

    fn gather_screen(&mut self, request: &Request, asked: Asked) {
        if request.body == 0 {
            return self.reply(request.id, 400, Some(r#"{"error":"empty"}"#));
        }
        let work = self.phone.w();
        work.text.clear();
        work.job = Job::ScreenBody {
            id: request.id,
            received: 0,
            asked,
        };
    }

    pub(super) fn progress_screen_body(&mut self, server: &mut impl Server, now: u64) {
        let Job::ScreenBody {
            id,
            received,
            asked,
        } = core::mem::replace(&mut self.phone.w().job, Job::Idle)
        else {
            return;
        };
        let Some(body) = server.request().filter(|r| r.id == id).map(|r| r.body) else {
            return;
        };
        let received = self.take_body(server, id, received, body, now);
        if received < body {
            self.phone.w().job = Job::ScreenBody {
                id,
                received,
                asked,
            };
            return;
        }
        if body as usize > TEXT {
            return self.reply(id, 413, Some(r#"{"error":"too_large"}"#));
        }
        let named = match asked {
            Asked::Check(named) | Asked::Save { named, .. } => named,
        };
        let (Some(work), Some(overrides)) =
            (self.phone.work.as_deref(), self.overrides.as_deref_mut())
        else {
            return;
        };
        let change = Change::Set {
            key: named.key(),
            nodes: &work.text,
        };
        let proposed = overrides.propose(change, self.device);
        if self.accepted(id, proposed) {
            match asked {
                Asked::Check(named) => self.list(id, Doc::Solves(named)),
                Asked::Save { base, .. } => self.save_screens(id, base),
            }
        }
    }

    fn change_screens(&mut self, id: u32, change: Change<'_>, base: Option<u32>) {
        let Some(base) = base else {
            return self.reply(id, 400, Some(r#"{"error":"base"}"#));
        };
        let Some(overrides) = self.overrides.as_deref_mut() else {
            return;
        };
        let proposed = overrides.propose(change, self.device);
        if self.accepted(id, proposed) {
            self.save_screens(id, base);
        }
    }

    /// Whether the change was accepted; if not, answer the reason.
    fn accepted(&mut self, id: u32, proposed: Result<(), Reject>) -> bool {
        let Err(reject) = proposed else {
            return true;
        };
        let mut json = heapless::String::<256>::new();
        let _ = json.push_str(r#"{"error":"refused","reason":""#);
        let _ = write!(Escaped(&mut json), "{reject}");
        let _ = json.push_str(r#""}"#);
        self.reply(id, 422, Some(json.as_str()));
        false
    }

    fn save_screens(&mut self, id: u32, base: u32) {
        if let Some(overrides) = self.overrides.as_deref_mut() {
            overrides.save(base);
            self.phone.w().job = Job::ScreenSave { id };
        }
    }

    pub(super) fn progress_screen_save(&mut self) {
        let Job::ScreenSave { id } = self.phone.w().job else {
            return;
        };
        let Some(saved) = self.overrides.as_deref_mut().and_then(|o| o.saved()) else {
            return;
        };
        let mut json = heapless::String::<48>::new();
        let status = match saved {
            Saved::Revision(revision) => {
                self.display.redraw();
                let _ = write!(json, r#"{{"revision":{revision}}}"#);
                200
            }
            Saved::Stale(revision) => {
                let _ = write!(json, r#"{{"error":"stale","revision":{revision}}}"#);
                409
            }
            Saved::Failed => {
                let _ = json.push_str(r#"{"error":"storage"}"#);
                503
            }
        };
        self.reply(id, status, Some(json.as_str()));
    }

    fn list(&mut self, id: u32, doc: Doc) {
        let mut open = heapless::String::<320>::new();
        self.head(doc, &mut open);
        self.open_json(&open);
        self.phone.w().job = Job::Listing {
            id,
            doc,
            next: 0,
            first: true,
        };
    }

    pub(super) fn progress_listing(&mut self, server: &mut impl Server) {
        let Job::Listing {
            id,
            doc,
            mut next,
            mut first,
        } = self.phone.w().job
        else {
            return;
        };
        if !self.send_text(server, id) {
            return;
        }
        loop {
            let mut item = heapless::String::<ITEM>::new();
            if !first {
                let _ = item.push(',');
            }
            match self.item(doc, next, &mut item) {
                Step::End => break,
                Step::Skip => next += 1,
                Step::Wrote if server.room() < item.len() => {
                    self.phone.w().job = Job::Listing {
                        id,
                        doc,
                        next,
                        first,
                    };
                    return;
                }
                Step::Wrote => {
                    server.write(id, item.as_bytes());
                    first = false;
                    next += 1;
                }
            }
        }
        if server.room() < 2 {
            self.phone.w().job = Job::Listing {
                id,
                doc,
                next,
                first,
            };
            return;
        }
        server.write(id, b"]}");
        server.finish(id);
        self.phone.w().job = Job::Idle;
    }

    fn head(&self, doc: Doc, out: &mut heapless::String<320>) {
        match doc {
            Doc::Screens => {
                let overrides = self.overrides.as_deref();
                let _ = write!(
                    out,
                    r#"{{"version":{VERSION},"revision":{},"reset":{},"screens":["#,
                    super::Maybe(overrides.and_then(|o| o.revision())),
                    overrides.is_some_and(|o| o.reset()),
                );
            }
            Doc::Catalogue => {
                let at = &self.device.scaled;
                let kind = |node: Node| node.bytes()[0];
                let leaf = Node::leaf(0, 0, Size::CONTENT);
                let _ = write!(
                    out,
                    r#"{{"version":{VERSION},"panel":[{},{}],"scale":{},"max_nodes":{MAX_NODES},"max_depth":{MAX_DEPTH},"kinds":{{"row":{},"col":{},"scroll":{},"stack":{},"leaf":{},"hero":{}}},"grow":{},"flags":{{"COURSE":{},"RADAR":{},"WORKOUT":{},"ARRIVED":{}}},"tiles":["#,
                    at.panel.width,
                    at.panel.height,
                    at.scale.px(4),
                    kind(Node::row(1, Size::CONTENT)),
                    kind(Node::col(1, Size::CONTENT)),
                    kind(Node::scroll(ListKind::Workouts, Size::CONTENT)),
                    kind(Node::stack(1, Size::CONTENT)),
                    kind(leaf),
                    kind(leaf.hero()) & !kind(leaf),
                    leaf.bytes()[2],
                    Flags::COURSE.bits(),
                    Flags::RADAR.bits(),
                    Flags::WORKOUT.bits(),
                    Flags::ARRIVED.bits(),
                );
            }
            Doc::Solves(named) => {
                let _ = write!(out, r#"{{"screen":"{}","solves":["#, named.name);
            }
        }
    }

    fn item(&self, doc: Doc, next: u16, out: &mut heapless::String<ITEM>) -> Step {
        let Some(overrides) = self.overrides.as_deref() else {
            return Step::End;
        };
        match doc {
            Doc::Screens => {
                let Some(named) = NAMED.get(usize::from(next)) else {
                    return Step::End;
                };
                let changed = overrides.shown().screen(named.key());
                let choices = Choices {
                    main_slots: self.preferences.options.main_slots,
                    ..Choices::default()
                };
                let builtin = named.builtin(&choices);
                let nodes = changed.map_or(&builtin[..], |screen| screen.tree().nodes());
                let _ = write!(
                    out,
                    r#"{{"name":"{}","key":{},"override":{},"nodes":""#,
                    named.name,
                    named.key(),
                    changed.is_some(),
                );
                for byte in nodes.iter().flat_map(|node| node.bytes()) {
                    let _ = write!(out, "{byte:02x}");
                }
                let _ = out.push_str(r#""}"#);
                Step::Wrote
            }
            Doc::Catalogue => {
                let arg = (next % 256) as u8;
                let Some((component, tile)) = registry::every().nth(usize::from(next / 256)) else {
                    return Step::End;
                };
                if !tile.accepts(arg) {
                    return Step::Skip;
                }
                let forms = tile.forms(arg, &self.device.scaled);
                if forms.is_empty() {
                    return Step::Skip;
                }
                let _ = write!(
                    out,
                    r#"{{"tile":"{}","id":{component},"arg":{arg},"label":""#,
                    tile.name()
                );
                let _ = tile.label(arg, &mut Escaped(out));
                let _ = write!(out, r#"","heavy":{},"forms":["#, tile.heavy());
                for (index, form) in forms.iter().enumerate() {
                    let comma = if index == 0 { "" } else { "," };
                    let _ = write!(out, "{comma}[{},{}]", form.width, form.height);
                }
                let _ = out.push_str("]}");
                Step::Wrote
            }
            Doc::Solves(named) => {
                let Some(screen) = overrides.proposed().screen(named.key()) else {
                    return Step::End;
                };
                let mentioned = screen
                    .tree()
                    .nodes()
                    .iter()
                    .fold(Flags::NONE, |flags, node| {
                        flags.with(node.condition().mentions())
                    });
                let Some(flags) = mentioned.subsets().nth(usize::from(next)) else {
                    return Step::End;
                };
                let mut solved = Solved::EMPTY;
                screen.solve(flags, &mut solved);
                let _ = write!(out, r#"{{"flags":"{flags}","leaves":["#);
                for (index, placed) in solved.leaves().iter().enumerate() {
                    let rect = placed.rect;
                    let comma = if index == 0 { "" } else { "," };
                    let _ = write!(
                        out,
                        "{comma}[{},{},{},{},{},{}]",
                        placed.node, placed.form, rect.left, rect.top, rect.right, rect.bottom
                    );
                }
                let _ = out.push_str("]}");
                Step::Wrote
            }
        }
    }
}

/// Writes text as the inside of a JSON string.
struct Escaped<'a, W>(&'a mut W);

impl<W: Write> Write for Escaped<'_, W> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        for c in text.chars() {
            match c {
                '"' => self.0.write_str("\\\"")?,
                '\\' => self.0.write_str("\\\\")?,
                c if (c as u32) < 0x20 => write!(self.0, "\\u{:04x}", c as u32)?,
                c => self.0.write_char(c)?,
            }
        }
        Ok(())
    }
}
