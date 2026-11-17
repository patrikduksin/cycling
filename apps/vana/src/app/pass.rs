use super::{Observations, Runtime};
use device_api::ant::Ant;
use device_api::ble_transport::Ble;
use device_api::bulk::ReadWrite;
use device_api::display::Display;
use device_api::input::{InputObservation, InputSource};
use device_api::observation::Observation;
use device_api::positioning::Positioning;
use device_api::power::{Power, PowerControl};
use device_api::sensors::{Light, Startup};
use device_api::server::{AccessPoint, Server};
use device_api::sound::Sound;
use device_api::storage::OwnedFlash;
use firmware_shell::shell::Shell;

/// Where a pass hands control back to composition, after the steps the name
/// says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Point {
    /// The shell ticked; its input source and synthetic edges are ready to record.
    InputsReady,
    Shell,
    ShellPresent,
    VanaTick,
    LookAhead,
    Phone,
    VanaPresent,
    VanaAfter,
}

pub struct Handles<A, Bl, Bu, Ap, Sv, So, Pc> {
    pub ant: A,
    pub ble: Bl,
    pub bulk: Bu,
    pub access_point: Ap,
    pub server: Sv,
    pub sound: So,
    pub control: Pc,
}

/// Inlined so the pass adds no frame under the frames VANA prepares.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
pub fn pass<D, I, P, B, Pos, In, A, Bl, Bu, Ap, Sv, So, Pc>(
    app: &mut Runtime,
    shell: &mut Shell<D, I, P, B>,
    startup: Startup,
    light: Observation<Light>,
    seen: Observations<'_, Pos, In>,
    now: u64,
    handles: Handles<A, Bl, Bu, Ap, Sv, So, Pc>,
    at: &mut impl FnMut(Point, &mut Shell<D, I, P, B>),
) where
    D: Display,
    I: InputSource,
    P: Power,
    B: OwnedFlash,
    Pos: Positioning,
    In: InputObservation,
    A: Ant,
    Bl: Ble,
    Bu: ReadWrite,
    Ap: AccessPoint,
    Sv: Server,
    So: Sound,
    Pc: PowerControl,
{
    let Handles {
        mut ant,
        mut ble,
        mut bulk,
        mut access_point,
        mut server,
        mut sound,
        mut control,
    } = handles;
    let position = seen.position;
    app.set_startup_status(startup);
    shell.set_app_active(app.input_active());
    shell.observe_light(light);
    shell.tick(now);
    at(Point::InputsReady, shell);
    for _ in 0..16 {
        let Some(edge) = shell.take_app_input() else {
            break;
        };
        app.input_received(edge.input, edge.received_ms, now, &mut ant);
    }
    shell.observe_position(position, now);
    at(Point::Shell, shell);
    shell.present();
    at(Point::ShellPresent, shell);
    app.tick(
        &mut shell.data_storage(),
        &mut bulk,
        now,
        &mut ant,
        &mut ble,
        seen,
    );
    app.service_format(shell, &mut bulk, now);
    at(Point::VanaTick, shell);
    // One bounded look-ahead step a pass (a tile chunk, a match or a
    // prediction), before the frame that may show its result.
    app.look_ahead(&mut bulk, now);
    at(Point::LookAhead, shell);
    app.serve_phone(
        &mut server,
        &mut access_point,
        &mut shell.data_storage(),
        &mut bulk,
        now,
    );
    at(Point::Phone, shell);
    app.present(shell, now, &ant, position);
    at(Point::VanaPresent, shell);
    app.save_preferences(shell, now);
    app.alert(&mut sound, now);
    app.power(shell, &mut control, now);
    at(Point::VanaAfter, shell);
}
