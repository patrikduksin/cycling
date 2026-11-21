//! A block of the summary's centred lines: a message, the first page's
//! heading, the best-power header, the FTP offer or DELETE RIDE?.
use crate::compose::data::Data;
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Forms, Tile};
use crate::screens::canvas::Canvas;
use crate::screens::ride::summary::{self, Words};
use crate::screens::scaled::Scaled;

pub struct Block;

impl Arg for Words {
    fn decode(raw: u8) -> Option<Self> {
        Self::ALL.get(usize::from(raw)).copied()
    }

    fn encode(self) -> u8 {
        self as u8
    }
}

impl Tile for Block {
    type Arg = Words;
    const NAME: &'static str = "words";

    fn forms(words: Words, at: &Scaled) -> Forms {
        summary::words_forms(words, at)
    }

    fn prepare(words: Words, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        if let Some(facts) = data.summary() {
            summary::words(words, placed.rect.rect(), at, &facts, out);
        }
    }
}
