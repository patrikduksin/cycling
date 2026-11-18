//! The internal flash of a replay: no medium of its own, every
//! access answered by the recorded world.
use crate::devices::Scripted;
use device_api::journal::{Aging, Get, MethodId, Put, Script, Stored};
use device_api::observation::{Availability, Error};
use device_api::storage::{Geometry, OwnedFlash, Region};
use firmware_services::trace::replayed::Replayed;
use firmware_services::trace::wire::{Wire as _, Writer};

pub struct ScriptedFlash {
    world: Scripted,
    availability: MethodId,
    geometry: MethodId,
    read: MethodId,
    program: MethodId,
    erase: MethodId,
}

impl ScriptedFlash {
    pub fn new(world: Scripted) -> Self {
        let method = |name| {
            super::script::named("OwnedFlash", name).expect("a method of the flash capability")
        };
        Self {
            world,
            availability: method("availability"),
            geometry: method("geometry"),
            read: method("read"),
            program: method("program"),
            erase: method("erase"),
        }
    }

    fn observed<T: Replayed + Aging>(&self, method: MethodId, region: Region) -> Option<T> {
        let mut key = [0u8; 1];
        region.put(&mut Writer::new(&mut key, 0));
        self.world.borrow_mut().observation(method, &key)
    }
}

const UNRECORDED_AVAILABILITY: Availability = Availability::Ready;

impl OwnedFlash for ScriptedFlash {
    type Error = Error;

    fn availability(&self, region: Region) -> Availability {
        self.observed(self.availability, region)
            .unwrap_or(UNRECORDED_AVAILABILITY)
    }

    fn geometry(&self, region: Region) -> Geometry {
        let mut world = self.world.borrow_mut();
        let script = &mut *world;
        script.begin(self.geometry);
        Put::put(script, &region);
        let at = script.observed();
        let geometry: Geometry = Get::get(script);
        script.end();
        geometry.aged(script.pass().now_ms.saturating_sub(at))
    }

    fn read(&mut self, region: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        let mut world = self.world.borrow_mut();
        Stored::read(&mut *world, self.read, (region, offset), out)
    }

    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        let mut world = self.world.borrow_mut();
        Stored::write(&mut *world, self.program, (region, offset), bytes)
    }

    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), Error> {
        let mut world = self.world.borrow_mut();
        Stored::erase(&mut *world, self.erase, (region, offset), length)
    }
}

/// The flash a simulated shell owns: the simulator's, or a replay's.
pub enum Flash {
    Simulated(crate::storage::Volatile<crate::storage::FileFlash>),
    Scripted(ScriptedFlash),
}

impl OwnedFlash for Flash {
    type Error = Error;

    fn availability(&self, region: Region) -> Availability {
        match self {
            Self::Simulated(flash) => flash.availability(region),
            Self::Scripted(flash) => flash.availability(region),
        }
    }

    fn geometry(&self, region: Region) -> Geometry {
        match self {
            Self::Simulated(flash) => flash.geometry(region),
            Self::Scripted(flash) => flash.geometry(region),
        }
    }

    fn read(&mut self, region: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        match self {
            Self::Simulated(flash) => flash.read(region, offset, out),
            Self::Scripted(flash) => flash.read(region, offset, out),
        }
    }

    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        match self {
            Self::Simulated(flash) => flash.program(region, offset, bytes),
            Self::Scripted(flash) => flash.program(region, offset, bytes),
        }
    }

    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), Error> {
        match self {
            Self::Simulated(flash) => flash.erase(region, offset, length),
            Self::Scripted(flash) => flash.erase(region, offset, length),
        }
    }
}
