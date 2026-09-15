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

pub mod power {
    pub struct Access;
    pub static ACCESS: Access = Access;
    impl Access {
        pub fn enter(&self) -> Result<(), device_api::ant::Error> {
            Ok(())
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
fn consumer_scan_receive_send_and_power_failures_preserve_ownership() {
    use device_api::ant::{
        Admission, Ant as _, AntOperation, Error, Identity, ScanState, SendStage, Target,
    };
    use device_api::power::PeripheralState;
    let mut ant = adapter::Ant;
    let ended = [0xf3, 3, 0, 0, 0, 0, 0, 0];
    assert_eq!(
        ant.request(AntOperation::Scan(5_000), 0),
        Ok(Admission::Accepted)
    );
    adapter::tick(1);
    assert_eq!(
        ant.request(AntOperation::StopScan, 1_000),
        Ok(Admission::Accepted)
    );
    adapter::tick(1_001);
    adapter::receive(16, ended, 1_002); // Explicit stop, timer still owns a future report.
    adapter::tick(3_001);
    assert_eq!(
        ant.request(AntOperation::Scan(5_000), 3_002),
        Err(Error::Busy)
    );
    assert_eq!(ant.scan().state, ScanState::Stopping);
    adapter::receive(16, ended, 6_001); // Previous natural timer is now drained.
    assert_eq!(
        ant.request(AntOperation::Scan(5_000), 7_000),
        Ok(Admission::Accepted)
    );
    adapter::tick(7_001);
    adapter::receive(16, ended, 7_002); // An early end cannot complete the new duration.
    assert!(ant.scanning());
    adapter::receive(16, ended, 13_001);
    assert_eq!(ant.scan().state, ScanState::Idle);

    let peer = Identity {
        device_type: 120,
        device_number: 123,
        transmission_type: 1,
    };
    assert_eq!(
        ant.request(AntOperation::Connect(peer), 14_000),
        Ok(Admission::Accepted)
    );
    adapter::tick(14_001);
    adapter::receive(1, [0x17, 120, 123, 0, 1, 3, 0, 0], 14_002);
    adapter::receive(120, [1; 8], 14_003);
    let observed = ant.take_diagnostic_packet().unwrap();
    assert_eq!(ant.take_packet(), Some(observed));
    let target = Target {
        identity: peer,
        generation: observed.generation,
    };
    let send = |data| AntOperation::Send {
        target,
        data: [data; 8],
    };
    let Ok(Admission::SendQueued(id)) = ant.request(send(2), 14_004) else {
        panic!("send rejected")
    };
    assert_eq!(ant.send_status(id).unwrap().stage, SendStage::Queued);
    assert_eq!(ant.request(send(3), 14_004), Err(Error::Busy));
    adapter::tick(14_005);
    assert_eq!(ant.send_status(id).unwrap().stage, SendStage::UartSubmitted);
    adapter::reply(120, [2, 2, 1, 0, 0, 0, 0, 0], 14_006);
    assert_eq!(
        ant.send_status(id).unwrap().stage,
        SendStage::BridgeReplied { accepted: true }
    );
    assert_eq!(ant.take_packet(), None); // A bridge reply is not a sensor response.
    assert_eq!(ant.request(send(2), 14_007), Err(Error::Uncertain));
    for value in 3..=8 {
        let Ok(Admission::SendQueued(_)) = ant.request(send(value), 14_008) else {
            panic!("send rejected")
        };
        adapter::tick(14_009);
        adapter::reply(120, [value, value, 1, 0, 0, 0, 0, 0], 14_010);
    }
    let Ok(Admission::SendQueued(cancelled)) = ant.request(send(9), 14_011) else {
        panic!("send rejected")
    };
    // Sleep cancels queued work before it reaches the UART, then closes the peer.
    adapter::power_request(true, 14_012).unwrap();
    assert_eq!(
        ant.send_status(cancelled).unwrap().stage,
        SendStage::Cancelled
    );
    adapter::tick(14_013);
    adapter::receive(1, [0x17, 120, 123, 0, 1, 4, 0, 0], 14_014);
    assert_eq!(adapter::power_status(), PeripheralState::Quiescent);
    adapter::power_request(false, 14_015).unwrap();
    adapter::tick(14_016);
    adapter::receive(1, [0x17, 120, 123, 0, 1, 3, 0, 0], 14_017);
    assert_eq!(adapter::power_status(), PeripheralState::Running);
    let resumed = Target {
        identity: peer,
        generation: ant.channel(120, 14_018).unwrap().generation,
    };
    assert_eq!(
        ant.request(
            AntOperation::Send {
                target: resumed,
                data: [10; 8]
            },
            14_018
        ),
        Err(Error::Capacity)
    );
    adapter::power_request(true, 14_019).unwrap();
    *drivers::companion_uart::FAIL_NEXT.lock().unwrap() = true;
    adapter::tick(14_020);
    assert_eq!(adapter::power_status(), PeripheralState::Failed);
    let submitted = drivers::companion_uart::SENT.lock().unwrap().len();
    adapter::tick(30_000);
    adapter::power_request(false, 30_001).unwrap();
    adapter::tick(30_002);
    assert_eq!(adapter::power_status(), PeripheralState::Failed);
    assert_eq!(
        drivers::companion_uart::SENT.lock().unwrap().len(),
        submitted
    );
}
