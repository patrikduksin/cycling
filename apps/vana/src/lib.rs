#![no_std]

#[cfg(all(feature = "planted-bug", target_os = "none"))]
compile_error!("planted-bug is the fuzzer's proof and never reaches a device");
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

pub mod app;
pub mod clock;
pub mod daylight;
pub mod kept;
pub mod ride;
pub mod screens;
pub mod sensors;
#[cfg(test)]
mod testing;
pub mod workout;
