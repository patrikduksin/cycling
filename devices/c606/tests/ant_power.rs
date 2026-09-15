//! Exercise the device ANT owner on the host with its production channel and
//! wire protocol. Only critical-section locking and UART access are substituted.
extern crate self as embassy_sync;

pub mod blocking_mutex {
    pub mod raw {
        pub struct CriticalSectionRawMutex;
    }

    pub struct Mutex<R, T>(std::sync::Mutex<T>, std::marker::PhantomData<R>);

    impl<R, T> Mutex<R, T> {
        pub const fn new(value: T) -> Self {
            Self(std::sync::Mutex::new(value), std::marker::PhantomData)
        }

        pub fn lock<U>(&self, f: impl FnOnce(&T) -> U) -> U {
            f(&self.0.lock().unwrap())
        }
    }
}

#[path = "../src/drivers/ant_protocol.rs"]
pub mod ant_protocol;

pub mod drivers {
    pub use crate::ant_protocol;
    pub use c606_firmware::drivers::companion;

    pub mod companion_uart {
        use std::sync::Mutex;

        pub static SENT: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
        pub static FAIL_NEXT: Mutex<bool> = Mutex::new(false);

        // Keep the production UART owner's signature.
        #[allow(clippy::result_unit_err)]
        pub fn send(frame: &[u8]) -> Result<(), ()> {
            SENT.lock().unwrap().push(frame.to_vec());
            if std::mem::take(&mut FAIL_NEXT.lock().unwrap()) {
                Err(())
            } else {
                Ok(())
            }
        }
    }
}

pub mod sensors {
    pub fn ant_allowed() -> bool {
        true
    }
}

#[allow(dead_code)]
#[path = "../src/capabilities/ant.rs"]
mod adapter;

#[test]
fn failed_uart_close_keeps_power_failed_without_replay() {
    use device_api::ant::AntOperation;
    use device_api::ant::Identity;
    use device_api::power::PeripheralState;
    let peer = Identity {
        device_type: 120,
        device_number: 123,
        transmission_type: 1,
    };
    assert_eq!(adapter::request(AntOperation::Connect(peer), 0), "ACCEPTED");
    adapter::tick(1);
    adapter::receive(1, [0x17, 120, 123, 0, 1, 3, 0, 0], 2);
    adapter::power_request(true, 3).unwrap();
    *drivers::companion_uart::FAIL_NEXT.lock().unwrap() = true;
    adapter::tick(4);
    assert_eq!(adapter::power_status(), PeripheralState::Failed);
    assert_eq!(drivers::companion_uart::SENT.lock().unwrap().len(), 2);
    adapter::tick(20_000);
    adapter::power_request(false, 20_001).unwrap();
    adapter::tick(20_002);
    assert_eq!(adapter::power_status(), PeripheralState::Failed);
    assert_eq!(drivers::companion_uart::SENT.lock().unwrap().len(), 2);
}
