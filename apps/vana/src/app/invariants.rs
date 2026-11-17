use super::Runtime;
use super::power::Sheet;
use super::recording::Medium;
use crate::ride::control::Action;
use crate::ride::recorder::invariants as recorder;
use crate::screens::workout::Page;

invariants! {
    pub enum Rule of runtime: Runtime {
        PhoneWhileRecording: "phone session open while recording"
            if runtime.phone_status().is_some() && runtime.recording(),
        TurningOffWhileRecording: "turning-off frame shown while recording"
            if matches!(runtime.power.sheet, Sheet::TurningOff { .. }) && runtime.recording(),
        ConfirmOffWhileRecording: "turn-off question shown while recording"
            if matches!(runtime.power.sheet, Sheet::Confirm { .. }) && runtime.recording(),
        TwoFtpQuestions: "two FTP questions asked"
            if runtime.training.ftp_offer.is_some() && runtime.offer_shown(),
        SavedSensorsDisagree: "saved sensors disagree with preferences"
            if runtime.links.saved != runtime.preferences.sensors,
        SummaryWhileRiding: "summary shown while riding"
            if runtime.navigation.page == Page::Summary
                && runtime.ride.active().riding()
                && runtime.ride.active().pending_action() != Some(Action::Finish),
        AutoLapDisagrees: "auto lap disagrees with options"
            if runtime.ride.auto_lap != runtime.preferences.options.auto_lap,
        StartupStatusDisagreesOnceKnown: "known startup status disagrees with startup"
            if runtime.startup_status != super::STARTUP_UNKNOWN
                && runtime.startup_status != runtime.startup.name(),
        TwoRidingRides: "both journals riding"
            if runtime.ride.internal.riding() && runtime.ride.bulk.riding(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Broken {
    Rule(Rule),
    Recorder(Medium, recorder::Rule),
}

impl core::fmt::Display for Broken {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Rule(rule) => write!(f, "{rule}"),
            Self::Recorder(medium, rule) => write!(f, "{} recorder: {rule}", medium.name()),
        }
    }
}

impl Runtime {
    pub fn broken(&self) -> Option<Broken> {
        [Medium::Internal, Medium::Bulk]
            .into_iter()
            .find_map(|medium| {
                recorder::Rule::of(self.ride.recorder(medium))
                    .map(|rule| Broken::Recorder(medium, rule))
            })
            .or_else(|| Rule::of(self).map(Broken::Rule))
    }

    #[cfg(any(test, feature = "invariants"))]
    #[doc(hidden)]
    pub fn break_auto_lap_rule(&mut self) {
        self.ride.auto_lap = match self.preferences.options.auto_lap {
            crate::ride::laps::AutoLap::Off => crate::ride::laps::AutoLap::AtStart,
            _ => crate::ride::laps::AutoLap::Off,
        };
    }
}
