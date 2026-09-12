//! Device composition adapter for the portable core store.
use crate::device::storage::{Backend, Error};
pub use cycling_os::storage::Loaded;

pub struct Store(cycling_os::storage::Store<Backend<'static>>);
impl Store {
    pub fn open(
        flash: esp_hal::peripherals::FLASH<'static>,
    ) -> (Self, Result<Loaded, cycling_os::storage::Error<Error>>) {
        let mut store = Self(cycling_os::storage::Store::new(Backend::new(flash)));
        let loaded = store.load();
        (store, loaded)
    }
}
impl core::ops::Deref for Store {
    type Target = cycling_os::storage::Store<Backend<'static>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl core::ops::DerefMut for Store {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
