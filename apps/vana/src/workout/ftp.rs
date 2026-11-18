use crate::kept::bests::suggest_ftp;
use crate::ride::zones::TrainingProfile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    WorkoutTest,
    RideSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub suggested_watts: u16,
    pub current_watts: u16,
    pub best_watts: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Question {
    Test { ftp: u16, current: u16 },
    Summary(Offer),
}

#[derive(Default)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
pub struct Candidates {
    test: Option<u16>,
    best: Option<u16>,
}

impl Candidates {
    pub fn set(&mut self, source: Source, watts: Option<u16>) {
        match source {
            Source::WorkoutTest => self.test = watts,
            Source::RideSummary => self.best = watts,
        }
    }

    pub fn answered(&mut self, source: Source) {
        self.set(source, None);
    }

    pub fn test(&self) -> Option<u16> {
        self.test
    }

    pub fn summary_offer(&self, current: u16) -> Option<Offer> {
        let best = self.best?;
        let suggested =
            suggest_ftp(best, current).filter(|ftp| TrainingProfile::FTP_WATTS.contains(ftp))?;
        Some(Offer {
            suggested_watts: suggested,
            current_watts: current,
            best_watts: best,
        })
    }

    pub fn question(&self, current: u16, on_offer_page: bool) -> Option<Question> {
        if let Some(ftp) = self.test {
            return Some(Question::Test { ftp, current });
        }
        self.summary_offer(current)
            .filter(|_| on_offer_page)
            .map(Question::Summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT: u16 = 200;
    const BEST: u16 = 260;

    fn both() -> Candidates {
        let mut candidates = Candidates::default();
        candidates.set(Source::WorkoutTest, Some(230));
        candidates.set(Source::RideSummary, Some(BEST));
        candidates
    }

    #[test]
    fn the_fixture_best_suggests_an_ftp() {
        assert_eq!(suggest_ftp(BEST, CURRENT), Some(247));
    }

    #[test]
    fn a_test_is_asked_before_the_summary_on_any_page() {
        let candidates = both();
        for on_offer_page in [false, true] {
            assert_eq!(
                candidates.question(CURRENT, on_offer_page),
                Some(Question::Test {
                    ftp: 230,
                    current: CURRENT
                })
            );
        }
    }

    #[test]
    fn the_summary_asks_on_its_page_once_the_test_is_answered() {
        let mut candidates = both();
        candidates.answered(Source::WorkoutTest);
        assert_eq!(candidates.question(CURRENT, false), None);
        assert_eq!(
            candidates.question(CURRENT, true),
            Some(Question::Summary(Offer {
                suggested_watts: 247,
                current_watts: CURRENT,
                best_watts: BEST
            }))
        );
        candidates.answered(Source::RideSummary);
        assert_eq!(candidates.question(CURRENT, true), None);
    }

    #[test]
    fn answering_one_keeps_the_other() {
        let mut candidates = both();
        candidates.answered(Source::RideSummary);
        assert_eq!(candidates.test(), Some(230));
        assert!(candidates.summary_offer(CURRENT).is_none());
    }

    #[test]
    fn a_best_too_close_to_the_ftp_in_force_offers_nothing() {
        let candidates = both();
        assert!(candidates.summary_offer(BEST).is_none());
    }
}
