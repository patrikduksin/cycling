//! Workout recording commands.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Start,
    Pause,
    Resume,
    Finish,
}
