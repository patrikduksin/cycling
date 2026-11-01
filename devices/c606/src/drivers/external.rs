//! A block of external RAM that gives its bytes back when dropped.
use core::alloc::Layout;
use core::ptr::NonNull;

pub type Free = unsafe fn(*mut u8, Layout);

pub struct Owned {
    pointer: NonNull<u8>,
    layout: Layout,
    free: Free,
}

impl Owned {
    /// The block at `pointer`; `None` when it is null.
    ///
    /// # Safety
    ///
    /// `pointer` holds `layout.size()` initialized bytes that nothing else
    /// references, and `free` gives them back with `layout`.
    pub unsafe fn from_raw(pointer: *mut u8, layout: Layout, free: Free) -> Option<Self> {
        Some(Self {
            pointer: NonNull::new(pointer)?,
            layout,
            free,
        })
    }
}

impl AsRef<[u8]> for Owned {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: `from_raw`'s caller handed over `layout.size()`
        // initialized bytes this block alone references.
        unsafe { core::slice::from_raw_parts(self.pointer.as_ptr(), self.layout.size()) }
    }
}

impl AsMut<[u8]> for Owned {
    fn as_mut(&mut self) -> &mut [u8] {
        // SAFETY: as in `as_ref`, and `&mut self` makes this the only
        // reference.
        unsafe { core::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.layout.size()) }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: `from_raw`'s caller promised `free` takes this block back
        // with `layout`; it is not used after this.
        unsafe { (self.free)(self.pointer.as_ptr(), self.layout) }
    }
}
