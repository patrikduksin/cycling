//! WAMR 2.4.6's fast interpreter, on bare metal and on the host alike: one
//! runtime a program. Modules and instances come from the pool [`init`] is
//! given, and linear memory from its `memories`.
//!
//! The runtime is one global and its platform layer has no locks: the
//! caller serialises every call into this crate.
#![no_std]

use core::cell::UnsafeCell;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::fmt::Write as _;
use core::ptr::NonNull;

/// What WAMR said when it refused a module or trapped.
pub type Message = heapless::String<96>;

/// A host function a module may import: WAMR's `NativeSymbol`. Its
/// signature uses WAMR's letters: `i` an i32, `*~` a pointer into the
/// guest's memory and the length after it, checked by WAMR before the call.
#[repr(C)]
pub struct Native {
    name: *const c_char,
    func: *mut c_void,
    signature: *const c_char,
    attachment: *mut c_void,
}

impl Native {
    /// `func` must be an `extern "C" fn` whose first argument is the
    /// execution environment and whose others match `signature`.
    pub const fn new(name: &'static CStr, func: *const (), signature: &'static CStr) -> Self {
        Self {
            name: name.as_ptr(),
            func: func as *mut c_void,
            signature: signature.as_ptr(),
            attachment: core::ptr::null_mut(),
        }
    }
}

/// One page of linear memory, the most an instance gets.
#[repr(C, align(16))]
pub struct Memory(pub [u8; PAGE]);

/// Bytes in a WebAssembly page.
pub const PAGE: usize = 64 * 1024;

/// Natives to [`register`], which WAMR sorts in place.
pub struct Natives<const N: usize>(UnsafeCell<[Native; N]>);

// SAFETY: only `register` reaches inside, under its caller's promise that
// nothing else calls into this crate meanwhile.
unsafe impl<const N: usize> Sync for Natives<N> {}

impl<const N: usize> Natives<N> {
    pub const fn new(natives: [Native; N]) -> Self {
        Self(UnsafeCell::new(natives))
    }
}

unsafe extern "C" {
    fn wamr_shim_init(pool: *mut u8, size: u32) -> c_int;
    fn wamr_shim_slabs(base: *mut u8, size: u32, count: u32) -> c_int;
    fn wamr_shim_export(module: *mut c_void, index: i32, name: *mut *const c_char) -> c_int;
    fn wamr_shim_starts(module: *mut c_void) -> bool;
    fn wamr_shim_trap(env: *mut c_void, why: *const c_char);
    fn wamr_shim_load(bytes: *const u8, len: u32, err: *mut c_char, errlen: u32) -> *mut c_void;
    fn wamr_shim_import(
        module: *mut c_void,
        index: i32,
        from: *mut *const c_char,
        name: *mut *const c_char,
        linked: *mut c_int,
    ) -> c_int;
    fn wamr_shim_instantiate(
        module: *mut c_void,
        stack: u32,
        pages: u32,
        err: *mut c_char,
        errlen: u32,
    ) -> *mut c_void;
    fn wamr_shim_call(
        env: *mut c_void,
        func: *mut c_void,
        argc: u32,
        argv: *mut u32,
        limit: c_int,
        err: *mut c_char,
        errlen: u32,
    ) -> bool;
    fn wamr_shim_pool_used() -> u32;
    fn wasm_runtime_register_natives(
        module: *const c_char,
        natives: *mut Native,
        count: u32,
    ) -> bool;
    fn wasm_runtime_unload(module: *mut c_void);
    fn wasm_runtime_deinstantiate(instance: *mut c_void);
    fn wasm_runtime_create_exec_env(instance: *mut c_void, stack: u32) -> *mut c_void;
    fn wasm_runtime_destroy_exec_env(env: *mut c_void);
    fn wasm_runtime_set_native_stack_boundary(env: *mut c_void, boundary: *mut u8);
    fn wasm_runtime_set_user_data(env: *mut c_void, data: *mut c_void);
    fn wasm_runtime_get_user_data(env: *mut c_void) -> *mut c_void;
    fn wasm_runtime_lookup_function(instance: *mut c_void, name: *const c_char) -> *mut c_void;
    fn wasm_func_get_param_count(func: *mut c_void, instance: *mut c_void) -> u32;
    fn wasm_func_get_result_count(func: *mut c_void, instance: *mut c_void) -> u32;
}

/// Start the runtime once a program: its allocations come from `pool`, and
/// each live instance takes one of `memories` (at most four). False if
/// WAMR did not start.
///
/// # Safety
///
/// No other call into this crate runs at the same time, and the runtime
/// owns `pool` and `memories` from here on.
pub unsafe fn init(pool: &'static mut [u8], memories: &'static mut [Memory]) -> bool {
    // SAFETY: the caller hands WAMR both for good.
    unsafe {
        wamr_shim_slabs(
            memories.as_mut_ptr().cast(),
            PAGE as u32,
            memories.len() as u32,
        ) == 0
            && wamr_shim_init(pool.as_mut_ptr(), pool.len() as u32) == 0
    }
}

/// Offer `natives` to modules that import from `module`, once each.
///
/// # Safety
///
/// [`init`] succeeded, nothing else calls into this crate meanwhile, and
/// each native's function matches its signature.
pub unsafe fn register<const N: usize>(
    module: &'static CStr,
    natives: &'static Natives<N>,
) -> bool {
    // SAFETY: as the caller promised; WAMR reads the array for as long as
    // the program runs.
    unsafe { wasm_runtime_register_natives(module.as_ptr(), natives.0.get().cast(), N as u32) }
}

/// Bytes of the pool in use.
pub fn pool_used() -> usize {
    // SAFETY: reads the allocator's totals; zero before `init`.
    unsafe { wamr_shim_pool_used() as usize }
}

/// The pointer [`Instance::call`] was given, read inside a native.
///
/// # Safety
///
/// `env` is the environment WAMR passed the native.
pub unsafe fn user_data(env: *mut c_void) -> *mut c_void {
    // SAFETY: as the caller promised.
    unsafe { wasm_runtime_get_user_data(env) }
}

/// Stop the call running in `env` with `why`, from inside a native.
///
/// # Safety
///
/// `env` is the environment WAMR passed the native.
pub unsafe fn trap(env: *mut c_void, why: &'static CStr) {
    // SAFETY: as the caller promised; WAMR copies the message.
    unsafe { wamr_shim_trap(env, why.as_ptr()) }
}

fn message(buffer: &[u8]) -> Message {
    let end = buffer
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(buffer.len());
    let text = core::str::from_utf8(&buffer[..end]).unwrap_or("unreadable message");
    let text = text.strip_prefix("Exception: ").unwrap_or(text);
    let mut out = Message::new();
    for c in text.chars() {
        if out.push(c).is_err() {
            break;
        }
    }
    out
}

/// A validated, loaded module.
pub struct Module(NonNull<c_void>);

/// One import a module names.
pub struct Import<'a> {
    pub from: &'a str,
    pub name: &'a str,
    pub function: bool,
    /// Whether a registered native answers it.
    pub linked: bool,
}

impl Module {
    /// Validate and load `bytes`, which may be dropped afterwards.
    ///
    /// # Safety
    ///
    /// [`init`] succeeded and nothing else calls into this crate meanwhile.
    pub unsafe fn load(bytes: &[u8]) -> Result<Self, Message> {
        let mut err = [0u8; 128];
        // SAFETY: the shim copies `bytes` into the pool before WAMR reads them.
        let module = unsafe {
            wamr_shim_load(
                bytes.as_ptr(),
                bytes.len() as u32,
                err.as_mut_ptr().cast(),
                err.len() as u32,
            )
        };
        NonNull::new(module).map(Self).ok_or_else(|| message(&err))
    }

    /// The import at `index`, or none past the last.
    pub fn import(&self, index: usize) -> Option<Import<'_>> {
        let (mut from, mut name, mut linked) = (core::ptr::null(), core::ptr::null(), 0);
        // SAFETY: the module is loaded; WAMR's names live as long as it.
        let kind = unsafe {
            wamr_shim_import(
                self.0.as_ptr(),
                index as i32,
                &mut from,
                &mut name,
                &mut linked,
            )
        };
        if kind < 0 {
            return None;
        }
        // SAFETY: WAMR's import names are NUL-terminated strings it owns.
        let text = |at: *const c_char| unsafe { CStr::from_ptr(at) }.to_str().unwrap_or("?");
        Some(Import {
            from: text(from),
            name: text(name),
            function: kind == 0,
            linked: linked != 0,
        })
    }
}

/// The exports WAMR runs as it instantiates a module, if they take and
/// return nothing; `_initialize` only for WASI, which this build leaves out.
const ENTRIES: [&str; 3] = ["__post_instantiate", "__wasm_call_ctors", "_initialize"];

impl Module {
    /// What of the module's own code instantiating it would run, outside
    /// any instruction limit: its start function or one of [`ENTRIES`].
    pub fn entry(&self) -> Option<&'static str> {
        // SAFETY: the module is loaded.
        if unsafe { wamr_shim_starts(self.0.as_ptr()) } {
            return Some("its start function");
        }
        ENTRIES.into_iter().find(|name| self.exports(name))
    }

    /// Whether the module exports a function `name`.
    pub fn exports(&self, name: &str) -> bool {
        let mut index = 0;
        loop {
            let mut at = core::ptr::null();
            // SAFETY: the module is loaded; WAMR's names live as long as it.
            let kind = unsafe { wamr_shim_export(self.0.as_ptr(), index, &mut at) };
            if kind < 0 {
                return false;
            }
            // SAFETY: a NUL-terminated name WAMR owns.
            if kind == 0 && unsafe { CStr::from_ptr(at) }.to_bytes() == name.as_bytes() {
                return true;
            }
            index += 1;
        }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // SAFETY: every instance of it was dropped first (see `Instance::new`).
        unsafe { wasm_runtime_unload(self.0.as_ptr()) }
    }
}

/// An instance of a module and the one environment that runs it.
pub struct Instance {
    instance: NonNull<c_void>,
    env: NonNull<c_void>,
}

/// An exported function and its shape.
#[derive(Clone, Copy)]
pub struct Func {
    func: NonNull<c_void>,
    pub params: u32,
    pub results: u32,
}

impl Instance {
    /// Instantiate `module` with an operand stack of `stack` bytes and at
    /// most `pages` pages of linear memory. A native below `floor` (the
    /// stack's lowest usable address) traps as a native stack overflow
    /// rather than run past it. A module with an [`entry`](Module::entry)
    /// is refused, so no guest code runs but through [`Instance::call`].
    ///
    /// # Safety
    ///
    /// [`init`] succeeded, nothing else calls into this crate meanwhile,
    /// and `module` outlives the instance.
    pub unsafe fn new(
        module: &Module,
        stack: u32,
        pages: u32,
        floor: Option<NonNull<u8>>,
    ) -> Result<Self, Message> {
        if let Some(entry) = module.entry() {
            let mut out = Message::new();
            let _ = write!(out, "runs {entry} when instantiated");
            return Err(out);
        }
        let mut err = [0u8; 128];
        // SAFETY: the module is loaded.
        let instance = unsafe {
            wamr_shim_instantiate(
                module.0.as_ptr(),
                stack,
                pages,
                err.as_mut_ptr().cast(),
                err.len() as u32,
            )
        };
        let instance = NonNull::new(instance).ok_or_else(|| message(&err))?;
        // SAFETY: the instance was just made.
        let env = unsafe { wasm_runtime_create_exec_env(instance.as_ptr(), stack) };
        let Some(env) = NonNull::new(env) else {
            // SAFETY: nothing else holds the instance.
            unsafe { wasm_runtime_deinstantiate(instance.as_ptr()) };
            let mut out = Message::new();
            let _ = write!(out, "no runtime memory for a {stack}-byte stack");
            return Err(out);
        };
        if let Some(floor) = floor {
            // SAFETY: the environment is live; WAMR only compares against it.
            unsafe { wasm_runtime_set_native_stack_boundary(env.as_ptr(), floor.as_ptr()) };
        }
        Ok(Self { instance, env })
    }

    pub fn func(&self, name: &CStr) -> Option<Func> {
        // SAFETY: the instance is live.
        let func = NonNull::new(unsafe {
            wasm_runtime_lookup_function(self.instance.as_ptr(), name.as_ptr())
        })?;
        // SAFETY: `func` belongs to this instance.
        let (params, results) = unsafe {
            (
                wasm_func_get_param_count(func.as_ptr(), self.instance.as_ptr()),
                wasm_func_get_result_count(func.as_ptr(), self.instance.as_ptr()),
            )
        };
        Some(Func {
            func,
            params,
            results,
        })
    }

    /// Run `func` with `args` (its results come back in `args`), stopped
    /// after `limit` instructions if given. Natives read `user` with
    /// [`user_data`].
    ///
    /// # Safety
    ///
    /// Nothing else calls into this crate meanwhile, `func` came from this
    /// instance, `args` holds at least as many words as its parameters and
    /// results, and `user` is what this instance's natives expect.
    pub unsafe fn call(
        &mut self,
        func: Func,
        args: &mut [u32],
        limit: Option<u32>,
        user: *mut c_void,
    ) -> Result<(), Message> {
        assert!(
            args.len() >= func.params.max(func.results) as usize,
            "too few argument words"
        );
        let limit = limit.map_or(-1, |limit| limit.min(i32::MAX as u32) as c_int);
        let mut err = [0u8; 128];
        // SAFETY: as the caller promised.
        let ran = unsafe {
            wasm_runtime_set_user_data(self.env.as_ptr(), user);
            let ran = wamr_shim_call(
                self.env.as_ptr(),
                func.func.as_ptr(),
                func.params,
                args.as_mut_ptr(),
                limit,
                err.as_mut_ptr().cast(),
                err.len() as u32,
            );
            wasm_runtime_set_user_data(self.env.as_ptr(), core::ptr::null_mut());
            ran
        };
        if ran { Ok(()) } else { Err(message(&err)) }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the environment and instance are this value's alone.
        unsafe {
            wasm_runtime_destroy_exec_env(self.env.as_ptr());
            wasm_runtime_deinstantiate(self.instance.as_ptr());
        }
    }
}
