//! How each value a trace carries is laid out: the [`Wire`] encoding of
//! supported device-api values, and of the trace's own records. A `u8` and
//! an enum's code are one byte; wider integers are varints, signed ones
//! zigzagged. Only fields written with [`Writer::time`] or [`Writer::times`]
//! are stored relative to the current pass.

mod device;
#[cfg(test)]
mod tests;

pub struct Writer<'b> {
    buf: &'b mut [u8],
    pub(super) len: usize,
    now: u64,
    pub(super) overflow: bool,
}

impl<'b> Writer<'b> {
    /// How much it wrote, or `None` when the buffer was too small.
    pub fn written(&self) -> Option<usize> {
        (!self.overflow).then_some(self.len)
    }

    /// A writer of a record body recorded in the pass at `now`.
    pub fn new(buf: &'b mut [u8], now: u64) -> Self {
        Self {
            buf,
            len: 0,
            now,
            overflow: false,
        }
    }

    pub fn byte(&mut self, byte: u8) {
        match self.buf.get_mut(self.len) {
            Some(slot) => {
                *slot = byte;
                self.len += 1;
            }
            None => self.overflow = true,
        }
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        match self.buf.get_mut(self.len..self.len + bytes.len()) {
            Some(slot) => {
                slot.copy_from_slice(bytes);
                self.len += bytes.len();
            }
            None => self.overflow = true,
        }
    }

    pub fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.byte(value as u8 | 0x80);
            value >>= 7;
        }
        self.byte(value as u8);
    }

    pub fn zigzag(&mut self, value: i64) {
        self.varint(((value << 1) ^ (value >> 63)) as u64);
    }

    pub(super) fn record(&mut self, tag: u8, body: impl FnOnce(&mut Self)) {
        self.byte(tag);
        let at = self.len;
        self.byte(0);
        body(self);
        if self.overflow {
            return;
        }
        let mut prefix = [0u8; 10];
        let mut p = Writer::new(&mut prefix, 0);
        p.varint((self.len - at - 1) as u64);
        let width = p.len;
        if self.len + width - 1 > self.buf.len() {
            self.overflow = true;
            return;
        }
        self.buf.copy_within(at + 1..self.len, at + width);
        self.buf[at..at + width].copy_from_slice(&prefix[..width]);
        self.len += width - 1;
    }

    pub fn time(&mut self, at: u64) {
        self.zigzag(self.now.wrapping_sub(at) as i64);
    }

    pub fn times(&mut self, at: Option<u64>) {
        self.byte(at.is_some().into());
        if let Some(at) = at {
            self.time(at);
        }
    }
}

pub struct Reader<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) at: usize,
    pub(super) now: u64,
    failed: bool,
}

impl<'a> Reader<'a> {
    /// A reader of a record body recorded in the pass at `now`.
    pub fn new(bytes: &'a [u8], now: u64) -> Self {
        Self {
            bytes,
            at: 0,
            now,
            failed: false,
        }
    }

    pub fn fail(&mut self) {
        self.failed = true;
    }

    pub(super) fn ok(&self) -> bool {
        !self.failed
    }

    /// How far into the body it has read, or `None` once a value did not
    /// decode.
    pub fn read(&self) -> Option<usize> {
        self.ok().then_some(self.at)
    }

    pub fn byte(&mut self) -> u8 {
        match self.bytes.get(self.at) {
            Some(&byte) => {
                self.at += 1;
                byte
            }
            None => {
                self.fail();
                0
            }
        }
    }

    pub fn bytes(&mut self, len: usize) -> &'a [u8] {
        match self.bytes.get(self.at..self.at.saturating_add(len)) {
            Some(slice) => {
                self.at += len;
                slice
            }
            None => {
                self.fail();
                &[]
            }
        }
    }

    pub fn varint(&mut self) -> u64 {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte();
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
        }
        self.fail();
        0
    }

    pub fn zigzag(&mut self) -> i64 {
        let value = self.varint();
        ((value >> 1) as i64) ^ -((value & 1) as i64)
    }

    pub fn time(&mut self) -> u64 {
        self.now.wrapping_sub(self.zigzag() as u64)
    }

    pub fn times(&mut self) -> Option<u64> {
        bool::take(self).then(|| self.time())
    }

    pub fn narrow<T: TryFrom<u64> + Default>(&mut self) -> T {
        let value = self.varint();
        T::try_from(value).unwrap_or_else(|_| {
            self.fail();
            T::default()
        })
    }
}

pub trait Wire<'a>: Sized {
    fn put(&self, w: &mut Writer);
    fn take(r: &mut Reader<'a>) -> Self;
}

impl Wire<'_> for u8 {
    fn put(&self, w: &mut Writer) {
        w.byte(*self);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        r.byte()
    }
}

macro_rules! unsigned {
    ($($ty:ty),*) => {$(
        impl Wire<'_> for $ty {
            fn put(&self, w: &mut Writer) {
                w.varint((*self).into());
            }
            fn take(r: &mut Reader<'_>) -> Self {
                r.narrow()
            }
        }
    )*};
}
unsigned!(u16, u32, u64);

macro_rules! signed {
    ($($ty:ty),*) => {$(
        impl Wire<'_> for $ty {
            fn put(&self, w: &mut Writer) {
                w.zigzag((*self).into());
            }
            fn take(r: &mut Reader<'_>) -> Self {
                let value = r.zigzag();
                <$ty>::try_from(value).unwrap_or_else(|_| {
                    r.fail();
                    0
                })
            }
        }
    )*};
}
signed!(i8, i16, i32);

impl Wire<'_> for bool {
    fn put(&self, w: &mut Writer) {
        w.byte((*self).into());
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => false,
            1 => true,
            _ => {
                r.fail();
                false
            }
        }
    }
}

impl<'a, T: Wire<'a>> Wire<'a> for Option<T> {
    fn put(&self, w: &mut Writer) {
        w.byte(self.is_some().into());
        if let Some(value) = self {
            value.put(w);
        }
    }
    fn take(r: &mut Reader<'a>) -> Self {
        bool::take(r).then(|| T::take(r))
    }
}

impl<'a, T: Wire<'a>, const N: usize> Wire<'a> for [T; N] {
    fn put(&self, w: &mut Writer) {
        for value in self {
            value.put(w);
        }
    }
    fn take(r: &mut Reader<'a>) -> Self {
        core::array::from_fn(|_| T::take(r))
    }
}

impl<'a, A: Wire<'a>, B: Wire<'a>> Wire<'a> for (A, B) {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        (A::take(r), B::take(r))
    }
}

impl<'a, A: Wire<'a>, B: Wire<'a>, C: Wire<'a>> Wire<'a> for (A, B, C) {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
        self.2.put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        (A::take(r), B::take(r), C::take(r))
    }
}

impl Wire<'_> for () {
    fn put(&self, _: &mut Writer) {}
    fn take(_: &mut Reader<'_>) -> Self {}
}

impl Wire<'_> for usize {
    fn put(&self, w: &mut Writer) {
        w.varint(*self as u64);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        r.narrow()
    }
}

impl<'a> Wire<'a> for &'a [u8] {
    fn put(&self, w: &mut Writer) {
        w.varint(self.len() as u64);
        w.bytes(self);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        let len = r.narrow();
        r.bytes(len)
    }
}

impl<'a> Wire<'a> for &'a str {
    fn put(&self, w: &mut Writer) {
        self.as_bytes().put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        let bytes = <&[u8]>::take(r);
        core::str::from_utf8(bytes).unwrap_or_else(|_| {
            r.fail();
            ""
        })
    }
}

macro_rules! record {
    ($($ty:path { $($field:ident $(: $kind:ident)?),* $(,)? })*) => {
        $(impl<'a> $crate::trace::wire::Wire<'a> for $ty {
            fn put(&self, w: &mut $crate::trace::wire::Writer) {
                $($crate::trace::wire::record!(@put w, self.$field $(, $kind)?);)*
            }
            fn take(r: &mut $crate::trace::wire::Reader<'a>) -> Self {
                Self { $($field: $crate::trace::wire::record!(@take r $(, $kind)?),)* }
            }
        })*
        #[cfg(test)]
        pub(crate) const RECORD_FIELDS: &[(&str, &[&str])] =
            &[$((stringify!($ty), &[$(stringify!($field)),*])),*];
    };
    (@put $w:ident, $value:expr) => { $crate::trace::wire::Wire::put(&$value, $w) };
    (@put $w:ident, $value:expr, time) => { $w.time($value) };
    (@put $w:ident, $value:expr, times) => { $w.times($value) };
    (@take $r:ident) => { $crate::trace::wire::Wire::take($r) };
    (@take $r:ident, time) => { $r.time() };
    (@take $r:ident, times) => { $r.times() };
}

macro_rules! codes {
    ($($ty:path { $first:ident = $first_code:literal $(, $variant:ident = $code:literal)* $(,)? })*) => {$(
        impl $crate::trace::wire::Wire<'_> for $ty {
            fn put(&self, w: &mut $crate::trace::wire::Writer) {
                w.byte(match self {
                    Self::$first => $first_code,
                    $(Self::$variant => $code,)*
                });
            }
            fn take(r: &mut $crate::trace::wire::Reader<'_>) -> Self {
                match r.byte() {
                    $first_code => Self::$first,
                    $($code => Self::$variant,)*
                    _ => {
                        r.fail();
                        Self::$first
                    }
                }
            }
        }
    )*};
}
pub(crate) use {codes, record};
