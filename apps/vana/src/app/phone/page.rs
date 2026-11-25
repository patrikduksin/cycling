//! The Phone page, which the sheet's Phone row opens: pairing from the
//! device alone. Unpaired, or after Pair new, it shows a code holding a new
//! pairing's URL for the iPhone Camera. That pending pairing seals the link
//! while the code is up and is saved only once a phone completes the
//! handshake with it; BACK, the timeout and leaving the page discard it.
//! Paired, it shows the paired page with Forget and Pair new.
use super::{Mode, Runtime};
use crate::app::navigation::To;
use crate::compose::input::OnPage;
use crate::screens::phone::Shown;
use crate::screens::sheet::PhoneRow;
use crate::screens::workout::Page;
use device_api::server::{Pairing, PhoneLink};
use firmware_services::qr;
use firmware_services::why;

pub const CODE_TIMEOUT_MS: u64 = 2 * 60 * 1000;
pub const TICK_MS: u64 = 3_000;

/// The code is held in place while it shows: the runtime has no heap.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Code {
        pending: Pairing,
        code: qr::Code,
        until: u64,
    },
    Done {
        until: u64,
    },
    Paired,
    Forget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Stay,
    Close,
    Commit(Pairing),
    Forget,
}

#[derive(Default)]
pub(super) struct Flow {
    state: Option<State>,
}

impl Flow {
    pub(super) fn pending(&self) -> Option<Pairing> {
        match self.state {
            Some(State::Code { pending, .. }) => Some(pending),
            _ => None,
        }
    }

    pub(super) fn full_light(&self) -> bool {
        matches!(self.state, Some(State::Code { .. }))
    }

    fn show_code(&mut self, pending: Pairing, now: u64) -> Option<()> {
        let code = qr::encode(super::pairing::url(&pending).as_bytes())?;
        self.state = Some(State::Code {
            pending,
            code,
            until: now.saturating_add(CODE_TIMEOUT_MS),
        });
        Some(())
    }

    fn open(&mut self, paired: bool, fresh: Option<Pairing>, now: u64) -> Step {
        if paired {
            self.state = Some(State::Paired);
            return Step::Stay;
        }
        match fresh.and_then(|fresh| self.show_code(fresh, now)) {
            Some(()) => Step::Stay,
            None => self.close(),
        }
    }

    fn close(&mut self) -> Step {
        self.state = None;
        Step::Close
    }

    fn act(&mut self, act: OnPage, fresh: Option<Pairing>, now: u64) -> Step {
        match (self.state, act) {
            (Some(State::Done { .. }), _) => self.close(),
            (Some(State::Paired), OnPage::Forget) => {
                self.state = Some(State::Forget);
                Step::Stay
            }
            (Some(State::Paired), OnPage::Pair) => self.open(false, fresh, now),
            (Some(State::Forget), OnPage::Answer(true)) => {
                self.state = None;
                Step::Forget
            }
            (Some(State::Forget), _) => {
                self.state = Some(State::Paired);
                Step::Stay
            }
            (_, OnPage::Back) => self.close(),
            _ => Step::Stay,
        }
    }

    fn linked(&mut self, link: PhoneLink, now: u64) -> Step {
        match self.state {
            Some(State::Code { pending, .. }) if link == PhoneLink::Connected => {
                self.state = Some(State::Done {
                    until: now.saturating_add(TICK_MS),
                });
                Step::Commit(pending)
            }
            _ => Step::Stay,
        }
    }

    /// The flow lives only while its page is in front: any other page closes
    /// it, as BACK does.
    fn tick(&mut self, now: u64, in_front: bool) -> Step {
        match self.state {
            Some(_) if !in_front => self.close(),
            Some(State::Code { until, .. } | State::Done { until }) if now >= until => self.close(),
            _ => Step::Stay,
        }
    }

    fn shown(&self, connected: bool) -> Option<Shown<'_>> {
        Some(match self.state.as_ref()? {
            State::Code { code, .. } => Shown::Code(code),
            State::Done { .. } => Shown::Done,
            State::Paired => Shown::Paired { connected },
            State::Forget => Shown::Forget,
        })
    }
}

impl Runtime {
    pub(in crate::app) fn open_phone_page(&mut self, now: u64) {
        if self.ride.recording() {
            why!("phone.page_while_riding");
            return;
        }
        let paired = self.phone.pairing.saved().is_some();
        let fresh = (!paired).then(|| self.phone.pairing.take_new()).flatten();
        let step = self.phone_flow(|flow| flow.open(paired, fresh, now));
        if self.phone.page.state.is_some() {
            self.navigation.go(To::Page(Page::Phone));
        }
        self.phone_step(step);
    }

    pub(in crate::app) fn phone_page_act(&mut self, act: OnPage, now: u64) {
        let fresh = matches!(act, OnPage::Pair)
            .then(|| self.phone.pairing.take_new())
            .flatten();
        let step = self.phone_flow(|flow| flow.act(act, fresh, now));
        self.phone_step(step);
    }

    pub(in crate::app) fn tick_phone_page(&mut self, now: u64) {
        let step = if self.ride.recording() && self.phone.page.state.is_some() {
            self.phone_flow(Flow::close)
        } else {
            let in_front = self.navigation.page() == Page::Phone;
            self.phone_flow(|flow| flow.tick(now, in_front))
        };
        self.phone_step(step);
    }

    pub(super) fn phone_page_linked(&mut self, link: PhoneLink, now: u64) {
        let step = self.phone_flow(|flow| flow.linked(link, now));
        self.phone_step(step);
    }

    pub(in crate::app) fn phone_row(&self) -> PhoneRow {
        match self.phone.pairing.saved() {
            None => PhoneRow::NotPaired,
            Some(_) if self.phone_connected() => PhoneRow::Connected,
            Some(_) => PhoneRow::Paired,
        }
    }

    fn phone_connected(&self) -> bool {
        self.phone.session.as_ref().is_some_and(|session| {
            session.status.mode == Mode::Bluetooth && session.status.link == PhoneLink::Connected
        })
    }

    pub(in crate::app) fn phone_shown(&self) -> Option<Shown<'_>> {
        self.phone.page.shown(self.phone_connected())
    }

    fn phone_flow(&mut self, step: impl FnOnce(&mut Flow) -> Step) -> Step {
        let before = self.sealing();
        let step = step(&mut self.phone.page);
        match step {
            Step::Commit(pairing) => self.phone.pairing.save(pairing),
            Step::Forget => self.phone.pairing.forget(),
            Step::Stay | Step::Close => {}
        }
        self.reseal(before);
        step
    }

    fn phone_step(&mut self, step: Step) {
        match step {
            Step::Stay => return,
            Step::Commit(pairing) => {
                why!("phone.code_paired", id = pairing.id);
                log::info!(target: "phone", "VANA_PHONE code_paired");
            }
            Step::Close | Step::Forget => {
                if self.navigation.page() == Page::Phone {
                    self.navigation.go(To::Home);
                }
            }
        }
        self.display.redraw();
    }

    pub(in crate::app) fn code_command(
        &mut self,
        now: u64,
        output: &mut impl core::fmt::Write,
    ) -> &'static str {
        if self.ride.recording() {
            return "BUSY";
        }
        let Some(fresh) = self.phone.pairing.take_new() else {
            return "BUSY";
        };
        self.close_sheet();
        let step = self.phone_flow(|flow| flow.open(false, Some(fresh), now));
        self.navigation.go(To::Page(Page::Phone));
        self.phone_step(step);
        match self.phone.page.pending() {
            Some(pending) => super::pairing::write_pairing(&pending, output),
            None => "FAILED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: Pairing = Pairing {
        id: [1, 1, 1, 1],
        key: [0x11; 32],
    };
    const NEW: Pairing = Pairing {
        id: [2, 2, 2, 2],
        key: [0x22; 32],
    };

    fn coded() -> Flow {
        let mut flow = Flow::default();
        assert_eq!(flow.open(false, Some(NEW), 0), Step::Stay);
        flow
    }

    #[test]
    fn unpaired_it_opens_on_a_code_whose_pairing_is_pending() {
        let flow = coded();
        assert_eq!(flow.pending(), Some(NEW));
        assert!(flow.full_light());
        let Some(Shown::Code(code)) = flow.shown(false) else {
            panic!("the code shows");
        };
        assert_eq!(
            *code,
            qr::encode(super::super::pairing::url(&NEW).as_bytes()).unwrap()
        );
    }

    #[test]
    fn a_completed_handshake_commits_the_pending_pairing_and_the_tick_closes_after_3_s() {
        let mut flow = coded();
        assert_eq!(flow.linked(PhoneLink::Advertising, 10), Step::Stay);
        assert_eq!(flow.linked(PhoneLink::Connected, 10), Step::Commit(NEW));
        assert_eq!(flow.pending(), None);
        assert_eq!(flow.shown(true), Some(Shown::Done));
        assert!(!flow.full_light());
        assert_eq!(flow.tick(10 + TICK_MS - 1, true), Step::Stay);
        assert_eq!(flow.tick(10 + TICK_MS, true), Step::Close);
        assert_eq!(flow.shown(true), None);
    }

    #[test]
    fn any_key_closes_the_tick() {
        for act in [OnPage::Back, OnPage::Forget, OnPage::Answer(true)] {
            let mut flow = coded();
            flow.linked(PhoneLink::Connected, 0);
            assert_eq!(flow.act(act, None, 1), Step::Close);
        }
    }

    #[test]
    fn back_and_the_timeout_discard_the_code_s_pairing() {
        let mut flow = coded();
        assert_eq!(flow.act(OnPage::Back, None, 1), Step::Close);
        assert_eq!(flow.pending(), None);
        let mut flow = coded();
        assert_eq!(flow.tick(CODE_TIMEOUT_MS - 1, true), Step::Stay);
        assert_eq!(flow.tick(CODE_TIMEOUT_MS, true), Step::Close);
        assert_eq!(flow.pending(), None);
        assert_eq!(
            flow.linked(PhoneLink::Connected, CODE_TIMEOUT_MS),
            Step::Stay
        );
    }

    #[test]
    fn another_page_in_front_closes_the_flow() {
        let mut flow = coded();
        assert_eq!(flow.tick(1, false), Step::Close);
        assert_eq!(flow.pending(), None);
        assert!(!flow.full_light());
        let mut flow = Flow::default();
        flow.open(true, None, 0);
        assert_eq!(flow.tick(1, false), Step::Close);
        assert_eq!(flow.shown(false), None);
    }

    #[test]
    fn paired_it_opens_on_the_paired_page_with_forget_and_pair_new() {
        let mut flow = Flow::default();
        assert_eq!(flow.open(true, None, 0), Step::Stay);
        assert_eq!(flow.shown(true), Some(Shown::Paired { connected: true }));
        assert_eq!(flow.pending(), None, "no code, the saved pairing stands");
        assert_eq!(flow.act(OnPage::Forget, None, 1), Step::Stay);
        assert_eq!(flow.shown(false), Some(Shown::Forget));
        assert_eq!(
            flow.act(OnPage::Back, None, 2),
            Step::Stay,
            "back to the page"
        );
        assert_eq!(flow.shown(false), Some(Shown::Paired { connected: false }));
        flow.act(OnPage::Forget, None, 3);
        assert_eq!(flow.act(OnPage::Answer(true), None, 4), Step::Forget);
        assert_eq!(flow.shown(false), None);

        let mut flow = Flow::default();
        flow.open(true, None, 0);
        assert_eq!(flow.act(OnPage::Pair, Some(NEW), 5), Step::Stay);
        assert_eq!(flow.pending(), Some(NEW), "Pair new shows a new code");
        assert_eq!(flow.act(OnPage::Back, Some(OLD), 6), Step::Close);
    }
}
