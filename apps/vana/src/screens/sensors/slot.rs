//! A selected sensor tile: the rider's intent combined with the observed link.
use super::{MenuMessage, connection_status};
use device_api::ant::{Identity, LinkState, Snapshot};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Operation {
    Connect,
    Disconnect,
}

#[derive(Clone, Copy)]
pub(super) struct Pending {
    pub(super) operation: Operation,
    /// The radio accepted the request; only then can an observation complete it.
    pub(super) admitted: bool,
}

/// What the rider should conclude about a selected sensor at a glance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LinkStatus {
    /// Connected with fresh data.
    Live,
    /// A request or link transition is in progress.
    Waiting,
    /// A request failed or the link timed out or lost its transport.
    Failed,
    /// Not connected and nothing in progress.
    Off,
    /// Connected, but data is missing or stale.
    Check,
}

impl LinkStatus {
    /// Prefix used when the status line also needs the peer's identity.
    pub(super) const fn prefix(self) -> &'static [u8] {
        match self {
            Self::Live => b"LIVE / ",
            Self::Waiting => b"WAIT / ",
            Self::Failed => b"FAILED / ",
            Self::Off => b"OFF / ",
            Self::Check => b"CHECK / ",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Selected {
    pub(super) identity: Identity,
    pub(super) observed: Option<Snapshot>,
    pub(super) pending: Option<Pending>,
    pub(super) failure: Option<MenuMessage>,
    pub(super) failed_disconnect: bool,
}

impl Selected {
    pub(super) const fn requested(identity: Identity, operation: Operation) -> Self {
        Self {
            identity,
            observed: None,
            pending: Some(Pending {
                operation,
                admitted: false,
            }),
            failure: None,
            failed_disconnect: false,
        }
    }

    pub(super) const fn observed(identity: Identity, snapshot: Snapshot) -> Self {
        Self {
            identity,
            observed: Some(snapshot),
            pending: None,
            failure: None,
            failed_disconnect: false,
        }
    }

    pub(super) fn link_status(self) -> LinkStatus {
        if self.failure.is_some() {
            return LinkStatus::Failed;
        }
        if self.pending.is_some() {
            return LinkStatus::Waiting;
        }
        let Some(snapshot) = self.observed else {
            return LinkStatus::Off;
        };
        match snapshot.link {
            LinkState::Idle | LinkState::Disconnected => LinkStatus::Off,
            LinkState::TimedOut | LinkState::TransportLost => LinkStatus::Failed,
            LinkState::Connecting | LinkState::Disconnecting => LinkStatus::Waiting,
            LinkState::Connected if !snapshot.stale && snapshot.age_ms.is_some() => {
                LinkStatus::Live
            }
            LinkState::Connected => LinkStatus::Check,
        }
    }

    /// Detailed wording for the focused sensor.
    pub(super) fn status_text(self) -> &'static [u8] {
        if let Some(failure) = self.failure {
            return failure.text().as_bytes();
        }
        match self.pending {
            Some(Pending {
                admitted: false, ..
            }) => b"WAITING FOR RADIO",
            Some(Pending {
                operation: Operation::Disconnect,
                ..
            }) => b"DISCONNECTING",
            Some(Pending {
                operation: Operation::Connect,
                ..
            }) => b"CONNECTING",
            None => self
                .observed
                .map_or(b"DISCONNECTED", |s| connection_status(s).as_bytes()),
        }
    }

    pub(super) fn retry(self) -> bool {
        matches!(self.link_status(), LinkStatus::Failed | LinkStatus::Off)
    }

    pub(super) fn can_disconnect(self) -> bool {
        self.observed.is_some_and(|s| {
            !matches!(
                s.link,
                LinkState::Idle | LinkState::Disconnected | LinkState::Disconnecting
            )
        })
    }

    /// Record an observation. Once admitted, a request completes when the link
    /// shows it took effect.
    pub(super) fn observe(&mut self, snapshot: Snapshot) {
        self.observed = Some(snapshot);
        let Some(Pending {
            operation,
            admitted: true,
        }) = self.pending
        else {
            return;
        };
        let completed = match operation {
            Operation::Disconnect => matches!(
                snapshot.link,
                LinkState::Disconnecting | LinkState::Disconnected | LinkState::TransportLost
            ),
            Operation::Connect => matches!(
                snapshot.link,
                LinkState::Connecting
                    | LinkState::Connected
                    | LinkState::TimedOut
                    | LinkState::TransportLost
            ),
        };
        if completed {
            self.pending = None;
        }
    }
}
