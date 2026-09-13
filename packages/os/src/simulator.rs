//! Deterministic owned devices for the portable shell. No physical timing claims.
use crate::capabilities::*;
use crate::storage::{OwnedFlash, Region};
use std::{cell::RefCell, rc::Rc, vec, vec::Vec};

#[derive(Clone)]
pub struct DisplayDevice(pub Rc<RefCell<Frame>>);
pub struct Frame {
    pub geometry: Geometry,
    pub pixels: Vec<u16>,
    pub submissions: u32,
    pub fail_next: bool,
}
impl DisplayDevice {
    pub fn new(width: usize, height: usize) -> Self {
        Self(Rc::new(RefCell::new(Frame {
            geometry: Geometry { width, height },
            pixels: vec![0; width * height],
            submissions: 0,
            fail_next: false,
        })))
    }
}
impl Display for DisplayDevice {
    fn geometry(&self) -> Geometry {
        self.0.borrow().geometry
    }
    fn submit(&mut self, pixel: impl Fn(usize, usize) -> u16) -> Result<(), Error> {
        let mut frame = self.0.borrow_mut();
        if core::mem::take(&mut frame.fail_next) {
            return Err(Error::Failed);
        }
        for y in 0..frame.geometry.height {
            for x in 0..frame.geometry.width {
                let offset = y * frame.geometry.width + x;
                frame.pixels[offset] = pixel(x, y);
            }
        }
        frame.submissions += 1;
        Ok(())
    }
}
#[derive(Clone)]
pub struct InputDevice {
    pub edges: Rc<RefCell<Edges>>,
    pub controls: Controls,
}
impl InputDevice {
    pub fn new(buttons: &'static [Button], touch: Availability) -> Self {
        Self {
            edges: Rc::new(RefCell::new(Edges::new())),
            controls: Controls { touch, buttons },
        }
    }
    pub fn push(&self, now: u64, event: Input) {
        self.edges.borrow_mut().push(now, event);
    }
}
impl InputSource for InputDevice {
    fn controls(&self) -> Controls {
        self.controls
    }
    fn take_edge(&mut self) -> Option<Edge> {
        self.edges.borrow_mut().pop()
    }
}
#[derive(Clone)]
pub struct PowerDevice(pub Rc<RefCell<PowerState>>);
pub struct PowerState {
    pub available: Availability,
    pub battery: Option<(u8, u16, u64)>,
    pub brightness: u8,
    pub writes: u32,
    pub fail: bool,
}
impl PowerDevice {
    pub fn new(available: Availability) -> Self {
        Self(Rc::new(RefCell::new(PowerState {
            available,
            battery: None,
            brightness: 0,
            writes: 0,
            fail: false,
        })))
    }
}
impl Power for PowerDevice {
    fn availability(&self) -> Availability {
        self.0.borrow().available
    }
    fn battery(&self) -> Option<(u8, u16, u64)> {
        self.0.borrow().battery
    }
    fn brightness(&mut self, percent: u8) -> Result<(), Error> {
        let mut state = self.0.borrow_mut();
        state.writes += 1;
        if state.available == Availability::Unsupported {
            return Err(Error::Unsupported);
        }
        if state.fail {
            return Err(Error::Failed);
        }
        state.brightness = percent;
        Ok(())
    }
}
/// The simulator has no positioning receiver or radios.
pub struct NoPosition;
impl Positioning for NoPosition {
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn snapshot(&self, _: u64) -> Option<crate::positioning::Snapshot> {
        None
    }
}
pub struct Memory {
    pub sectors: [[u8; 4096]; 2],
    pub available: Availability,
    pub fail: bool,
}
impl Default for Memory {
    fn default() -> Self {
        Self {
            sectors: [[0xff; 4096]; 2],
            available: Availability::Ready,
            fail: false,
        }
    }
}
impl Memory {
    fn check(&self) -> Result<(), Error> {
        match self.available {
            Availability::Unsupported => Err(Error::Unsupported),
            Availability::Ready if !self.fail => Ok(()),
            _ => Err(Error::Failed),
        }
    }
}
impl OwnedFlash for Memory {
    type Error = Error;
    fn availability(&self, region: Region) -> Availability {
        if region == Region::Configuration {
            self.available
        } else {
            Availability::Unsupported
        }
    }
    fn geometry(&self, region: Region) -> crate::storage::Geometry {
        crate::storage::Geometry {
            capacity: if region == Region::Configuration {
                8192
            } else {
                0
            },
            program_size: 4,
            erase_size: 4096,
        }
    }
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        if region != Region::Configuration {
            return Err(Error::Unsupported);
        }
        self.check()?;
        crate::storage::checked_range::<()>(
            self.geometry(region).capacity,
            offset,
            output.len(),
            1,
        )
        .map_err(|_| Error::Invalid)?;
        for (i, byte) in output.iter_mut().enumerate() {
            *byte = self.sectors[(offset + i) / 4096][(offset + i) % 4096];
        }
        Ok(())
    }
    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        if region != Region::Configuration {
            return Err(Error::Unsupported);
        }
        self.check()?;
        crate::storage::checked_range::<()>(self.geometry(region).capacity, offset, bytes.len(), 4)
            .map_err(|_| Error::Invalid)?;
        for (i, byte) in bytes.iter().enumerate() {
            self.sectors[(offset + i) / 4096][(offset + i) % 4096] &= *byte;
        }
        Ok(())
    }
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), Error> {
        if region != Region::Configuration {
            return Err(Error::Unsupported);
        }
        self.check()?;
        crate::storage::checked_range::<()>(self.geometry(region).capacity, offset, length, 4096)
            .map_err(|_| Error::Invalid)?;
        for i in offset..offset + length {
            self.sectors[i / 4096][i % 4096] = 0xff;
        }
        Ok(())
    }
}
