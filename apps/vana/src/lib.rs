#![no_std]

#[cfg(all(feature = "planted-bug", target_os = "none"))]
compile_error!("planted-bug is the fuzzer's proof and never reaches a device");
#[cfg(all(feature = "planted-reading-bug", target_os = "none"))]
compile_error!("planted-reading-bug is the fixtures' proof and never reaches a device");
#[cfg(all(feature = "invariants", target_os = "none"))]
compile_error!("invariants checks run in the simulator and never reach a device");

#[cfg(any(test, feature = "invariants"))]
extern crate std;

#[cfg(any(test, feature = "invariants"))]
macro_rules! invariants {
    ($vis:vis enum $name:ident of $this:ident: $owner:ty {
        $($rule:ident: $text:literal if $broken:expr,)+
    }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        $vis enum $name {
            $($rule,)+
        }

        impl $name {
            #[cfg(test)]
            pub(crate) const ALL: &[Self] = &[$(Self::$rule,)+];

            $vis fn of($this: &$owner) -> Option<Self> {
                $(if $broken {
                    return Some(Self::$rule);
                })+
                None
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(match self {
                    $(Self::$rule => $text,)+
                })
            }
        }
    };
}

macro_rules! state {
    ($owner:ty = $name:literal) => {
        #[cfg(not(target_os = "none"))]
        impl firmware_services::why::State for $owner {
            fn fields(&self, out: &mut firmware_services::why::Fields<'_>) {
                out.field($name, self);
            }
        }
    };
    ($owner:ty { $($field:ident),+ $(,)? }) => {
        #[cfg(not(target_os = "none"))]
        impl firmware_services::why::State for $owner {
            fn fields(&self, out: &mut firmware_services::why::Fields<'_>) {
                $(out.field(::core::stringify!($field), &self.$field);)+
            }
        }
    };
}

pub mod app;

/// What a device calls itself: the FIT file's product and the phone's
/// device name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Product {
    /// At most 15 bytes: the FIT product-name field holds 16 with its end.
    pub name: &'static str,
    pub fit_id: u16,
}

pub mod clock;
pub mod daylight;
pub mod kept;
pub mod ride;
pub mod screens;
pub mod sensors;
#[cfg(test)]
mod testing;
pub mod workout;
