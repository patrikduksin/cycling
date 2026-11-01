//! The C606 records what its panel, backlight, internal flash and card
//! refused or failed through `firmware_services::trace::shell` and a counted
//! `RecordedBulk`; the simulator's devices record it from their own state.
//! A replay fails the operation a fault names, so both must name the same
//! one.
use core::cell::RefCell;
use device_api::bulk::ReadWrite;
use device_api::display::Display;
use device_api::observation::Availability;
use device_api::power::Power;
use device_api::storage::{OwnedFlash, Region};
use firmware_services::trace::frozen::{CompanionView, Record, RecordedBulk};
use firmware_services::trace::shell::{PassLog, ShellDisplay, ShellFlash, ShellPower};
use firmware_services::trace::{Event, Output, StorageFault};
use shell_simulator::bulk::{BulkFault, SimBulk};
use shell_simulator::devices::{DisplayDevice, PowerDevice};
use shell_simulator::storage::FileFlash;

#[derive(Default)]
struct Kept(Vec<String>, Vec<StorageFault>);

impl Record for Kept {
    fn record(&mut self, event: &Event<'_>) {
        if let Event::StorageFault(fault) = event {
            self.1.push(*fault);
        }
        self.0.push(format!("{event:?}"));
    }
}

fn log() -> &'static PassLog {
    Box::leak(Box::default())
}

fn landed_unknown(faults: Vec<StorageFault>) -> Vec<StorageFault> {
    faults
        .into_iter()
        .map(|fault| StorageFault { landed: 0, ..fault })
        .collect()
}

#[test]
fn a_refused_frame_and_backlight_write_have_the_numbers_the_simulator_gives_them() {
    let log = log();
    let display = DisplayDevice::new(4, 4);
    let power = PowerDevice::new(Availability::Ready);
    let mut panel = ShellDisplay::new(display.clone(), log);
    let mut backlight = ShellPower::new(power.clone(), log);
    let kept = RefCell::new(Kept::default());
    for pass in 0..2 {
        display.0.borrow_mut().refusals.begin_pass();
        power.0.borrow_mut().refusals.begin_pass();
        log.begin_pass(Availability::Ready, &CompanionView(None));
        display.0.borrow_mut().refusals.scripted = vec![1];
        power.0.borrow_mut().refusals.scripted = vec![0, 2];
        for _ in 0..3 {
            let _ = panel.submit(|_, row| row.fill(pass));
            let _ = backlight.brightness(40);
        }
        log.record(&kept);
    }
    let simulated = |output: Output, refused: Vec<u8>| {
        refused
            .into_iter()
            .map(move |attempt| format!("{:?}", Event::Refused { output, attempt }))
    };
    let mut expected = Vec::new();
    for _ in 0..2 {
        expected.extend(simulated(Output::Panel, vec![1]));
        expected.extend(simulated(Output::Backlight, vec![0, 2]));
    }
    assert_eq!(kept.borrow().0, expected);
    assert_eq!(
        display.0.borrow().refusals.refused,
        [1, 1],
        "the simulator refused the same frames"
    );
}

#[test]
fn the_shell_reads_the_battery_and_backlight_the_pass_froze() {
    let log = log();
    let power = PowerDevice::new(Availability::Ready);
    power.0.borrow_mut().battery = Some((10, 3_400, 0));
    let backlight = ShellPower::new(power.clone(), log);
    let snapshot = device_api::input::InputSnapshot {
        battery: device_api::observation::Observation::Fresh {
            value: (64, 3_900),
            received_ms: 1_000,
        },
        power: device_api::observation::Observation::Unavailable,
        button_counts: [0; 3],
        touch_available: false,
        touch_errors: 0,
        companion_valid: 0,
        companion_bad_crc: 0,
        uart_errors: 0,
        input_lost: 0,
    };
    log.begin_pass(Availability::Failed, &CompanionView(Some(snapshot)));
    power.0.borrow_mut().available = Availability::Ready;
    assert_eq!(backlight.battery(), Some((64, 3_900, 1_000)));
    assert_eq!(backlight.availability(), Availability::Failed);
}

#[test]
fn a_failed_flash_operation_has_the_number_the_simulator_gives_it() {
    let log = log();
    let flash = FileFlash::memory();
    let mut recorded = ShellFlash::new(flash.clone(), log);
    let kept = RefCell::new(Kept::default());
    flash.fault.borrow_mut().after = Some(1);
    for pass in 0..2u8 {
        flash.fault.borrow_mut().begin_pass();
        log.begin_pass(Availability::Ready, &CompanionView(None));
        let mut bytes = [0u8; 8];
        let _ = recorded.read(Region::Data, 0, &mut bytes);
        let _ = recorded.erase(Region::Data, 4096, 4096);
        let _ = recorded.program(Region::Data, 4096 + 8 * usize::from(pass), &[pass; 8]);
        let _ = recorded.program(Region::Configuration, 0, &[7; 4]);
        log.record(&kept);
    }
    let simulated = core::mem::take(&mut flash.fault.borrow_mut().log);
    assert!(!simulated.is_empty(), "the simulator failed an operation");
    assert_eq!(kept.borrow().1, landed_unknown(simulated));
}

#[test]
fn a_failed_card_operation_has_the_number_the_simulator_gives_it() {
    let log = log();
    let card = SimBulk::default();
    card.fail(BulkFault {
        after: 2,
        partial: 100,
    });
    let kept = RefCell::new(Kept::default());
    let sector = [5u8; 512];
    card.begin_pass();
    log.begin_pass(Availability::Ready, &CompanionView(None));
    for binding in 0..2 {
        let mut inner = card.clone();
        let mut bulk = RecordedBulk::counted(&mut inner, &kept, log);
        let _ = bulk.owned_info();
        let _ = bulk.owned_write(100 + binding, &sector);
        let _ = bulk.owned_write(200 + binding, &sector);
    }
    log.record(&kept);
    let simulated = card.take_log();
    assert!(!simulated.is_empty(), "the simulator failed an operation");
    assert_eq!(kept.borrow().1, landed_unknown(simulated));
}

#[test]
fn a_pass_records_refusals_and_faults_in_the_order_the_simulator_does() {
    let log = log();
    let display = DisplayDevice::new(4, 4);
    let power = PowerDevice::new(Availability::Ready);
    let flash = FileFlash::memory();
    let card = SimBulk::default();
    let mut panel = ShellDisplay::new(display.clone(), log);
    let mut backlight = ShellPower::new(power.clone(), log);
    let mut recorded_flash = ShellFlash::new(flash.clone(), log);
    let kept = RefCell::new(Kept::default());
    display.0.borrow_mut().refusals.scripted = vec![0];
    power.0.borrow_mut().refusals.scripted = vec![0];
    flash.fault.borrow_mut().after = Some(0);
    card.fail(BulkFault {
        after: 0,
        partial: 0,
    });
    log.begin_pass(Availability::Ready, &CompanionView(None));
    let mut inner = card.clone();
    let _ = RecordedBulk::counted(&mut inner, &kept, log).owned_write(70, &[1; 512]);
    let _ = recorded_flash.program(Region::Data, 0, &[1; 4]);
    let _ = backlight.brightness(10);
    let _ = panel.submit(|_, row| row.fill(1));
    log.record(&kept);
    let flash_fault = flash.fault.borrow().log[0];
    let card_fault = card.take_log()[0];
    let simulated: Vec<String> = [
        Event::Refused {
            output: Output::Panel,
            attempt: 0,
        },
        Event::Refused {
            output: Output::Backlight,
            attempt: 0,
        },
        Event::StorageFault(StorageFault {
            landed: 0,
            ..flash_fault
        }),
        Event::StorageFault(StorageFault {
            landed: 0,
            ..card_fault
        }),
    ]
    .iter()
    .map(|event| format!("{event:?}"))
    .collect();
    assert_eq!(kept.borrow().0, simulated);
}
