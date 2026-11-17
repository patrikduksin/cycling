//! Declarations of how device methods are recorded.
//!
//! Every device-api trait carries `#[capability(id = N)]`, and every method
//! carries its kind. Arguments carry a role where one applies; an argument
//! without one is part of the call's canonical identity. The attribute
//! emits the capability's table beside the trait. The table's `ty` and
//! `returns` are display strings for reading, not for parsing. It also
//! emits the trait for `&mut T` and, for a recorded capability, a
//! `<Trait>Recorder` that records each call into a
//! [`Journal`](crate::journal::Journal).
//!
//! ```
//! use device_api::capability::capability;
//!
//! #[capability(id = 200)]
//! pub trait Lamp {
//!     #[observe]
//!     fn level(&self) -> u8;
//!     #[request(refreshes)]
//!     fn set(&mut self, level: u8, #[clock] now: u64) -> bool;
//! }
//! assert_eq!(CAPABILITY_LAMP.methods.len(), 2);
//! ```

use crate::{ant, ble_transport, bulk, connectivity, console, display, input};
use crate::{position_control, positioning};
use crate::{power, sensors, server, sound, storage};

pub use ::capability::capability;

/// A method can't have a body: it would exist unrecorded.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe]
///     fn level(&self) -> u8 { 0 }
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe]
///     fn level(&self) -> u8;
/// }
/// ```
///
/// A method needs a kind.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     fn level(&self) -> u8;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe]
///     fn level(&self) -> u8;
/// }
/// ```
///
/// An unknown kind is refused.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[watch]
///     fn level(&self) -> u8;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe]
///     fn level(&self) -> u8;
/// }
/// ```
///
/// An unknown argument role is refused.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn set(&mut self, level: u8, #[time] now: u64) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn set(&mut self, level: u8, #[clock] now: u64) -> bool;
/// }
/// ```
///
/// An observe only reads, so it takes `&self`.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe]
///     fn level(&mut self) -> u8;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe]
///     fn level(&self) -> u8;
/// }
/// ```
///
/// A request acts on the device, so it takes `&mut self`.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn set(&self, level: u8) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn set(&mut self, level: u8) -> bool;
/// }
/// ```
///
/// A typed receiver is refused: the receiver is `&self` or `&mut self`.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe]
///     fn level(self: &Self) -> u8;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe]
///     fn level(&self) -> u8;
/// }
/// ```
///
/// `#[out]` marks a `&mut` buffer.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[take]
///     fn read(&mut self, #[out] out: &[u8]) -> usize;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[take]
///     fn read(&mut self, #[out] out: &mut [u8]) -> usize;
/// }
/// ```
///
/// `#[input]` marks a `&` buffer.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn write(&mut self, #[input] bytes: &mut [u8]) -> usize;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn write(&mut self, #[input] bytes: &[u8]) -> usize;
/// }
/// ```
///
/// `#[out]` belongs to a take or a media read.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn read(&mut self, #[out] out: &mut [u8]) -> usize;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[take]
///     fn read(&mut self, #[out] out: &mut [u8]) -> usize;
/// }
/// ```
///
/// `#[input]` belongs to a sink or a media write.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[take]
///     fn write(&mut self, #[input] bytes: &[u8]) -> usize;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn write(&mut self, #[input] bytes: &[u8]) -> usize;
/// }
/// ```
///
/// `#[at]` belongs to a media method.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn seek(&mut self, #[at] sector: u64) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn seek(&mut self, sector: u64) -> bool;
/// }
/// ```
///
/// `#[len]` belongs to a media erase.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[media(write)]
///     fn put(&mut self, #[at] at: u64, #[input] bytes: &[u8], #[len] len: usize) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[media(write)]
///     fn put(&mut self, #[at] at: u64, #[input] bytes: &[u8]) -> bool;
/// }
/// ```
///
/// A media method marks its address with `#[at]`.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[media(erase)]
///     fn erase(&mut self, at: u64, #[len] len: usize) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[media(erase)]
///     fn erase(&mut self, #[at] at: u64, #[len] len: usize) -> bool;
/// }
/// ```
///
/// A media read takes exactly one `#[out]` buffer.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[media(read)]
///     fn read(&mut self, #[at] at: u64) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[media(read)]
///     fn read(&mut self, #[at] at: u64, #[out] out: &mut [u8]) -> bool;
/// }
/// ```
///
/// A borrowed argument without a role is refused: it is a buffer.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn write(&mut self, bytes: &[u8]) -> usize;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn write(&mut self, #[input] bytes: &[u8]) -> usize;
/// }
/// ```
///
/// Only a sink or an unrecorded method takes a callback.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn draw(&mut self, row: impl FnMut(usize)) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn draw(&mut self, row: impl FnMut(usize)) -> bool;
/// }
/// ```
///
/// A callback takes no role.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn draw(&mut self, #[input] row: impl FnMut(usize)) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn draw(&mut self, row: impl FnMut(usize)) -> bool;
/// }
/// ```
///
/// A method takes one callback.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn draw(&mut self, row: impl FnMut(usize), done: impl FnMut()) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn draw(&mut self, row: impl FnMut(usize), done: bool) -> bool;
/// }
/// ```
///
/// An argument has one role.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn set(&mut self, #[clock] #[clock] now: u64) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn set(&mut self, #[clock] now: u64) -> bool;
/// }
/// ```
///
/// An unkeyed observe takes no key argument.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe]
///     fn level(&self, channel: u8) -> u8;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe(keyed)]
///     fn level(&self, channel: u8) -> u8;
/// }
/// ```
///
/// `refreshes` belongs to a take, a request or a sink.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[command(refreshes)]
///     fn next(&mut self) -> Option<u8>;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[take(refreshes)]
///     fn next(&mut self) -> Option<u8>;
/// }
/// ```
///
/// Empty parentheses are refused.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request()]
///     fn set(&mut self, level: u8) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn set(&mut self, level: u8) -> bool;
/// }
/// ```
///
/// An option is given once.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request(refreshes, refreshes)]
///     fn set(&mut self, level: u8) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request(refreshes, redact)]
///     fn set(&mut self, level: u8) -> bool;
/// }
/// ```
///
/// A request that `repeats` is recorded when it differs from its last
/// record or changes an observation, so it `refreshes`.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request(repeats)]
///     fn keep(&mut self, level: u8) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request(refreshes, repeats)]
///     fn keep(&mut self, level: u8) -> bool;
/// }
/// ```
///
/// A method exists in every build: `#[cfg]` would leave the table listing a method the trait lacks.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[cfg(test)]
///     #[observe]
///     fn level(&self) -> u8;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[allow(unused)]
///     #[observe]
///     fn level(&self) -> u8;
/// }
/// ```
///
/// So does an argument.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn set(&mut self, #[cfg(test)] level: u8) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[request]
///     fn set(&mut self, #[allow(unused)] level: u8) -> bool;
/// }
/// ```
///
/// A method returns a named type: an `impl Trait` can't be recorded.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[derived]
///     fn next(&self) -> impl Iterator<Item = u8>;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[derived]
///     fn next(&self) -> Option<u8>;
/// }
/// fn derived_next(_: &(impl Lamp + ?Sized)) -> Option<u8> {
///     None
/// }
/// ```
///
/// A derived method is never recorded, so it has nothing to redact.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[derived(redact)]
///     fn level(&self) -> u8;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe(redact)]
///     fn level(&self) -> u8;
/// }
/// ```
///
/// An unrecorded method says why.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[unrecorded("")]
///     fn check(&mut self, row: impl FnMut(usize));
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[unrecorded("a simulator's self-check")]
///     fn check(&mut self, row: impl FnMut(usize));
/// }
/// ```
///
/// A redacted method's types stay out of `LampRecorded`, so a secret needs no encoding.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[derive(Clone, PartialEq)]
/// pub struct Secret;
/// impl device_api::journal::Aging for Secret {}
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe]
///     fn secret(&self) -> Secret;
///     #[observe]
///     fn level(&self) -> u8;
/// }
/// trait Encoded {}
/// impl Encoded for u8 {}
/// trait Each {}
/// impl Each for () {}
/// impl<T: Encoded, Rest: Each> Each for (T, Rest) {}
/// fn encoded<T: Each>() {}
/// encoded::<LampRecorded>();
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[derive(Clone, PartialEq)]
/// pub struct Secret;
/// impl device_api::journal::Aging for Secret {}
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[observe(redact)]
///     fn secret(&self) -> Secret;
///     #[observe]
///     fn level(&self) -> u8;
/// }
/// trait Encoded {}
/// impl Encoded for u8 {}
/// trait Each {}
/// impl Each for () {}
/// impl<T: Encoded, Rest: Each> Each for (T, Rest) {}
/// fn encoded<T: Each>() {}
/// encoded::<LampRecorded>();
/// ```
///
/// An `impl` argument is a callback, so it is an `impl FnMut`.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn show(&mut self, bytes: impl AsRef<[u8]>) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[sink]
///     fn show(&mut self, row: impl FnMut(usize, &mut [u16])) -> bool;
/// }
/// ```
///
/// Every argument of a media method takes a role.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[media(read)]
///     fn read(&mut self, #[at] at: u64, #[out] out: &mut [u8], tries: u8) -> bool;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200)]
/// pub trait Lamp {
///     #[media(read)]
///     fn read(&mut self, #[at] at: u64, #[out] out: &mut [u8]) -> bool;
/// }
/// ```
///
/// An outside capability says why.
///
/// ```compile_fail
/// # use device_api::capability::capability;
/// #[capability(id = 200, outside = "")]
/// pub trait Lamp {
///     fn read(&mut self) -> Option<u8>;
/// }
/// ```
///
/// ```
/// # use device_api::capability::capability;
/// #[capability(id = 200, outside = "a byte pipe")]
/// pub trait Lamp {
///     fn read(&mut self) -> Option<u8>;
/// }
/// ```
#[cfg(doctest)]
mod refusals {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability {
    /// Stable across builds.
    pub id: u8,
    pub name: &'static str,
    pub world: World,
    /// In declaration order. An outside capability lists none.
    pub methods: &'static [Method],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum World {
    Recorded,
    /// Not recorded, for the reason given.
    Outside(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Method {
    pub name: &'static str,
    pub kind: Kind,
    /// A secret: the trace replaces its values per call, so its types are
    /// left out of the capability's recorded types.
    pub redacted: bool,
    pub args: &'static [Arg],
    /// The return type as written.
    pub returns: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arg {
    pub name: &'static str,
    pub role: Role,
    /// The type as written.
    pub ty: &'static str,
}

/// A call that `refreshes` starts a new epoch of the capability's
/// observations as soon as it returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A value read without effect, sampled once per observation epoch.
    Observe(Keys),
    /// An item consumed from the device, matched one to one.
    Take {
        refreshes: bool,
    },
    /// An effect on the device, matched by identity. One that `repeats` is
    /// made pass after pass with the same arguments: a call is recorded
    /// only when it differs from the method's last recorded call, or
    /// changes what the capability observes.
    Request {
        refreshes: bool,
        repeats: bool,
    },
    /// Output to the device, recorded as a digest and its outcome.
    Sink {
        refreshes: bool,
    },
    Media(Media),
    /// A console command and its reply.
    Command,
    /// Computed from other methods of the same capability by the free
    /// function `derived_<method>` beside the trait; never recorded.
    Derived,
    /// Forwarded to the device and never recorded, for the reason given.
    Unrecorded(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keys {
    Unkeyed,
    /// The keys VANA asks for, sampled when it first asks.
    Asked,
    /// Every key, from the named enumerator, sampled each epoch.
    All(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    Read,
    Write,
    Erase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Identity,
    /// The caller's time; never recorded, outside the identity.
    Clock,
    /// Part of a media address.
    At,
    /// A buffer the device fills.
    Out,
    /// A buffer VANA hands in.
    Input,
    /// The length of a media erase.
    Len,
    /// An `impl FnMut` the device calls back.
    Callback,
}

macro_rules! registry {
    ($($module:ident::$table:ident $recorded:ident),* $(,)?) => {
        /// Every capability, by id.
        pub const ALL: &[&Capability] = &[$(&$module::$table),*];

        /// Declared value types checked for trace encoding, grouped by
        /// capability as nested pairs.
        pub type Recorded = registry!(@pairs $($module $recorded)*);
    };
    (@pairs) => { () };
    (@pairs $module:ident $recorded:ident $($rest:ident)*) => {
        ($module::$recorded, registry!(@pairs $($rest)*))
    };
}

registry! {
    ant::CAPABILITY_ANT AntRecorded,
    ble_transport::CAPABILITY_BLE BleRecorded,
    bulk::CAPABILITY_READ ReadRecorded,
    bulk::CAPABILITY_READ_WRITE ReadWriteRecorded,
    console::CAPABILITY_CONSOLE ConsoleRecorded,
    display::CAPABILITY_DISPLAY DisplayRecorded,
    input::CAPABILITY_INPUT_SOURCE InputSourceRecorded,
    input::CAPABILITY_INPUT_OBSERVATION InputObservationRecorded,
    position_control::CAPABILITY_POSITION_CONTROL PositionControlRecorded,
    positioning::CAPABILITY_POSITIONING PositioningRecorded,
    power::CAPABILITY_POWER PowerRecorded,
    power::CAPABILITY_POWER_CONTROL PowerControlRecorded,
    sensors::CAPABILITY_SENSORS SensorsRecorded,
    server::CAPABILITY_ACCESS_POINT AccessPointRecorded,
    server::CAPABILITY_SERVER ServerRecorded,
    sound::CAPABILITY_SOUND SoundRecorded,
    storage::CAPABILITY_OWNED_FLASH OwnedFlashRecorded,
    connectivity::CAPABILITY_WIFI WifiRecorded,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Region;
    use std::path::PathBuf;
    use std::string::{String, ToString};
    use std::vec::Vec;
    use std::{format, fs, vec};

    fn declared(method: &Method) -> String {
        let (kind, refreshes, mut options) = match method.kind {
            Kind::Observe(Keys::Unkeyed) => ("observe", false, Vec::new()),
            Kind::Observe(Keys::Asked) => ("observe", false, vec!["keyed".into()]),
            Kind::Observe(Keys::All(keys)) => ("observe", false, vec![format!("keys = {keys}")]),
            Kind::Take { refreshes } => ("take", refreshes, Vec::new()),
            Kind::Request { refreshes, .. } => ("request", refreshes, Vec::new()),
            Kind::Sink { refreshes } => ("sink", refreshes, Vec::new()),
            Kind::Media(Media::Read) => ("media", false, vec!["read".into()]),
            Kind::Media(Media::Write) => ("media", false, vec!["write".into()]),
            Kind::Media(Media::Erase) => ("media", false, vec!["erase".into()]),
            Kind::Command => ("command", false, Vec::new()),
            Kind::Derived => ("derived", false, Vec::new()),
            Kind::Unrecorded(why) => ("unrecorded", false, vec![format!("{why:?}")]),
        };
        options.extend(refreshes.then(|| "refreshes".into()));
        if let Kind::Request { repeats: true, .. } = method.kind {
            options.push("repeats".into());
        }
        options.extend(method.redacted.then(|| "redact".into()));
        let mut out = format!("{} {kind}", method.name);
        if !options.is_empty() {
            out += &format!("({})", options.join(", "));
        }
        for (role, name) in [
            (Role::Out, "out"),
            (Role::Input, "input"),
            (Role::Callback, "callback"),
        ] {
            if method.args.iter().any(|arg| arg.role == role) {
                out += &format!(" [{name}]");
            }
        }
        out
    }

    #[test]
    fn ids_and_names_are_unique() {
        let ids: Vec<u8> = ALL.iter().map(|capability| capability.id).collect();
        assert!(
            ids.is_sorted_by(|a, b| a < b),
            "ids are unique and ALL is in id order: {ids:?}"
        );
        for capability in ALL {
            let mut names: Vec<&str> = capability
                .methods
                .iter()
                .map(|method| method.name)
                .collect();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), capability.methods.len(), "{}", capability.name);
            match capability.world {
                World::Recorded => assert!(!names.is_empty(), "{}", capability.name),
                World::Outside(_) => assert!(names.is_empty(), "{}", capability.name),
            }
        }
    }

    #[test]
    fn every_capability_in_the_source_is_registered() {
        let mut declared = Vec::new();
        let mut dirs = vec![PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
        while let Some(dir) = dirs.pop() {
            for path in fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().path())
            {
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.ends_with("tests.rs") {
                    continue;
                }
                let mut marked = false;
                for line in fs::read_to_string(&path).unwrap().lines() {
                    let line = line.trim_start();
                    marked |= line.starts_with("#[capability(");
                    if let Some((_, rest)) = line.split_once("trait ").filter(|_| marked) {
                        let name = rest.split(|c: char| !c.is_alphanumeric() && c != '_');
                        declared.push(name.take(1).collect::<String>());
                        marked = false;
                    }
                }
            }
        }
        declared.sort();
        let mut registered: Vec<String> = ALL
            .iter()
            .map(|capability| capability.name.to_string())
            .collect();
        registered.sort();
        assert_eq!(declared, registered);
    }

    #[test]
    fn region_all_holds_the_regions_in_declaration_order() {
        let index = |region| match region {
            Region::Configuration => 0,
            Region::Data => 1,
            Region::Crash => 2,
            Region::Intent => 3,
        };
        for (position, region) in Region::ALL.into_iter().enumerate() {
            assert_eq!(index(region), position, "{region:?}");
        }
    }

    #[test]
    fn every_method_declares_how_it_is_recorded() {
        let table: Vec<String> = ALL
            .iter()
            .map(|capability| {
                let methods: Vec<String> = capability.methods.iter().map(declared).collect();
                match capability.world {
                    World::Recorded => format!(
                        "{} {}: {}",
                        capability.id,
                        capability.name,
                        methods.join(", ")
                    ),
                    World::Outside(why) => {
                        format!("{} {}: outside, {why}", capability.id, capability.name)
                    }
                }
            })
            .collect();
        assert_eq!(
            table.join("\n"),
            "\
1 Ant: scanning observe, discoveries observe, request request(refreshes), capabilities observe, scan observe, send_status observe(keyed), take_diagnostic_packet take, channel derived, availability observe, channels observe, take_packet take
2 Ble: request request(refreshes), control observe, discoveries observe, availability observe, snapshot observe, take_packet take, take_battery take
3 Read: info observe, read media(read) [out], clock request(refreshes), recover request(refreshes)
4 ReadWrite: owned_info request, owned_read media(read) [out], owned_write media(write) [input], entropy take
5 Console: outside, a byte pipe for the composition's terminal task; its lines enter the world as commands
6 Display: availability observe, geometry observe, format observe, submit sink [callback], check unrecorded(\"a simulator's self-check; a panel ignores it\") [callback]
7 InputSource: controls observe, take_edge take
8 InputObservation: snapshot observe
9 PositionControl: control observe, pause request(refreshes), resume request(refreshes), diagnostics observe
10 Positioning: availability observe, snapshot observe
11 Power: availability observe, battery observe, brightness sink
12 PowerControl: capabilities observe, status observe, request request(refreshes), request_after request(refreshes)
13 Sensors: startup_status observe, startup_reason observe, snapshot observe, light observe, identity_status observe, query_identity request(refreshes)
14 AccessPoint: start request(refreshes, redact), stop request(refreshes), state observe, reach observe, entropy take(redact)
15 Server: listen request(refreshes, repeats), request observe(redact), read take(refreshes) [out], room observe, write sink(refreshes) [input], finish request(refreshes)
16 Sound: patterns observe, snapshot observe, play request(refreshes), stop request(refreshes)
17 OwnedFlash: availability observe(keys = Region::ALL), geometry observe(keys = Region::ALL), read media(read) [out], program media(write) [input], erase media(erase)
18 Wifi: request request(refreshes, redact), control observe, discoveries observe, state observe, online observe, stats observe"
        );
    }
}
