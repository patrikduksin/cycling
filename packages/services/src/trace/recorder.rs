//! The recorder of a trace: it encodes events into blocks and hands each
//! sealed block to a sink.
use super::{Encoder, Event, Full};

/// Where a pass's events go.
pub trait Record {
    fn record(&mut self, event: &Event<'_>);
}

/// Watches a recorder from outside the trace: every event before its sink
/// can refuse the block.
#[cfg(feature = "testing")]
pub trait Tap {
    fn event(&mut self, event: &Event<'_>);
}

/// Where sealed event blocks go, in order.
pub trait Sink {
    /// Take a sealed block; false when there is no room for it, and it is
    /// lost.
    fn block(&mut self, bytes: &[u8]) -> bool;
}

/// Encodes events into blocks and hands each full block to its sink. The
/// header goes ahead of them, written by whoever writes the trace; see
/// [`super::encode_header`].
pub struct Recorder<B, S> {
    encoder: Encoder<B>,
    sink: S,
    lost: Lost,
    quiet: Option<u64>,
    #[cfg(feature = "testing")]
    tap: Option<std::boxed::Box<dyn Tap>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Lost {
    blocks: u32,
    bytes: u32,
}

impl<B: AsMut<[u8]> + AsRef<[u8]>, S: Sink> Recorder<B, S> {
    /// `buf` holds one block, of 64 to [`super::MAX_BLOCK`] bytes.
    pub fn new(buf: B, sink: S) -> Self {
        Self {
            encoder: Encoder::new(buf),
            sink,
            lost: Lost::default(),
            quiet: None,
            #[cfg(feature = "testing")]
            tap: None,
        }
    }

    #[cfg(feature = "testing")]
    pub fn tap(&mut self, tap: std::boxed::Box<dyn Tap>) {
        self.tap = Some(tap);
    }

    /// A pass that is recorded only when something else is recorded in it:
    /// its pass event waits for the first event, and goes when the next
    /// pass begins with none.
    pub fn quiet_pass(&mut self, now_ms: u64) {
        self.quiet = Some(now_ms);
    }

    /// The block being filled, framed, or nothing while it holds no event.
    /// It stays open.
    pub fn open_block(&mut self) -> &[u8] {
        if self.encoder.is_empty() {
            return &[];
        }
        self.encoder.sealed()
    }

    /// Hand the block being filled to the sink now, so that a reader of the
    /// sink is at most this old; nothing when it holds no event.
    pub fn flush(&mut self) {
        if !self.encoder.is_empty() {
            self.seal();
        }
    }

    /// The open block's buffer as memory holds it: what a crash leaves of
    /// it (see [`super::tail::Remains`]).
    pub fn open_memory(&self) -> &[u8] {
        self.encoder.memory()
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    fn push(&mut self, event: &Event<'_>) -> Result<(), Full> {
        let result = self.encoder.push(event);
        if result != Err(Full::Block) {
            return result;
        }
        self.seal();
        self.encoder.push(event)
    }

    /// Hand the open block to the sink and open the next one, which says
    /// first what the sink refused since it last took a block.
    fn seal(&mut self) {
        let block = self.encoder.sealed();
        let len = u32::try_from(block.len()).unwrap_or(u32::MAX);
        if self.sink.block(block) {
            self.lost = Lost::default();
        } else {
            self.lost.blocks = self.lost.blocks.saturating_add(1);
            self.lost.bytes = self.lost.bytes.saturating_add(len);
        }
        self.encoder.restart();
        if self.lost != Lost::default() {
            let Lost { blocks, bytes } = self.lost;
            let _ = self.encoder.push(&Event::Lost { blocks, bytes });
        }
    }
}

impl<B: AsMut<[u8]> + AsRef<[u8]>, S: Sink> Record for Recorder<B, S> {
    fn record(&mut self, event: &Event<'_>) {
        if let Some(now_ms) = self.quiet.take()
            && !matches!(event, Event::Pass { .. })
        {
            self.record(&Event::Pass { now_ms });
        }
        #[cfg(feature = "testing")]
        if let Some(tap) = &mut self.tap {
            tap.event(event);
        }
        if self.push(event) == Err(Full::Record) {
            let (capability, operation) = event.id();
            let _ = self.push(&Event::TooLarge {
                capability,
                operation,
            });
        }
        if event.seals() {
            self.flush();
        }
    }
}

impl<T: Record> Record for Option<T> {
    fn record(&mut self, event: &Event<'_>) {
        if let Some(record) = self {
            record.record(event);
        }
    }
}
