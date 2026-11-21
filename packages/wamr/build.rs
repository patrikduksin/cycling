//! Builds WAMR 2.4.6's fast interpreter and this package's bare-metal
//! platform layer. The source is the dev shell's `CYCLING_WAMR`, pinned by
//! hash in nix/shell.nix. The same C builds for the host and the device, so
//! the simulator validates, meters and traps exactly as the device does.
use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=c");
    println!("cargo::rerun-if-env-changed=CYCLING_WAMR");
    let wamr = PathBuf::from(
        std::env::var("CYCLING_WAMR")
            .expect("CYCLING_WAMR names WAMR's source: enter the dev shell (nix develop)"),
    );
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let bare = std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "none";
    let (target, invoke) = match arch.as_str() {
        "xtensa" => ("BUILD_TARGET_XTENSA", "invokeNative_xtensa.s"),
        "x86_64" => ("BUILD_TARGET_X86_64", "invokeNative_em64.s"),
        "aarch64" => ("BUILD_TARGET_AARCH64", "invokeNative_aarch64.s"),
        other => panic!("WAMR is not built for {other} here"),
    };
    let mut build = cc::Build::new();
    build
        .opt_level_str("s")
        .flag("-ffunction-sections")
        .flag("-fdata-sections")
        .flag("-fno-common")
        .warnings(false)
        .define(target, "1");
    if arch == "xtensa" {
        build.flag("-mlongcalls");
    }
    if bare {
        build.define("VANA_NO_LIBC", "1");
    }
    for (name, value) in [
        ("BH_PLATFORM_VANA", "1"),
        ("BH_MALLOC", "wasm_runtime_malloc"),
        ("BH_FREE", "wasm_runtime_free"),
        ("WASM_ENABLE_INTERP", "1"),
        ("WASM_ENABLE_FAST_INTERP", "1"),
        ("WASM_ENABLE_AOT", "0"),
        ("WASM_ENABLE_JIT", "0"),
        ("WASM_ENABLE_FAST_JIT", "0"),
        ("WASM_ENABLE_LIBC_BUILTIN", "0"),
        ("WASM_ENABLE_LIBC_WASI", "0"),
        ("WASM_ENABLE_MULTI_MODULE", "0"),
        ("WASM_ENABLE_THREAD_MGR", "0"),
        ("WASM_ENABLE_SHARED_MEMORY", "0"),
        // memory.fill and memory.copy, and table.fill, table.copy and
        // table.grow, do a whole memory's or table's work as one metered
        // instruction, so the loader refuses them.
        ("WASM_ENABLE_BULK_MEMORY", "0"),
        ("WASM_ENABLE_REF_TYPES", "0"),
        ("WASM_ENABLE_SIMD", "0"),
        ("WASM_ENABLE_GC", "0"),
        ("WASM_ENABLE_MEMORY64", "0"),
        ("WASM_ENABLE_TAIL_CALL", "0"),
        ("WASM_ENABLE_INSTRUCTION_METERING", "1"),
        ("WASM_ENABLE_LABELS_AS_VALUES", "1"),
        ("WASM_ENABLE_MINI_LOADER", "0"),
        ("WASM_ENABLE_DUMP_CALL_STACK", "0"),
        ("WASM_ENABLE_PERF_PROFILING", "0"),
        ("WASM_ENABLE_MEMORY_PROFILING", "0"),
        ("WASM_DISABLE_HW_BOUND_CHECK", "1"),
        ("WASM_DISABLE_STACK_HW_BOUND_CHECK", "1"),
        ("WASM_DISABLE_WAKEUP_BLOCKING_OP", "1"),
        ("WASM_ENABLE_WAMR_COMPILER", "0"),
        ("WASM_ENABLE_LOAD_CUSTOM_SECTION", "0"),
        ("WASM_ENABLE_CUSTOM_NAME_SECTION", "0"),
        ("BH_ENABLE_TRACE_MMAP", "0"),
        ("WASM_ENABLE_SPEC_TEST", "0"),
    ] {
        build.define(name, value);
    }
    for include in [
        "core/iwasm/include",
        "core/iwasm/common",
        "core/iwasm/interpreter",
        "core/shared/utils",
        "core/shared/mem-alloc",
        "core/shared/platform/include",
    ] {
        build.include(wamr.join(include));
    }
    build.include("c");
    let skip = ["wasm_application.c", "wasm_c_api.c"];
    for dir in [
        "core/iwasm/common",
        "core/shared/utils",
        "core/shared/mem-alloc",
        "core/shared/mem-alloc/ems",
    ] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(wamr.join(dir))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "c")
                    && !skip.iter().any(|name| path.ends_with(name))
            })
            .collect();
        files.sort();
        build.files(files);
    }
    for file in ["wasm_loader.c", "wasm_runtime.c", "wasm_interp_fast.c"] {
        build.file(wamr.join("core/iwasm/interpreter").join(file));
    }
    build.file(wamr.join("core/iwasm/common/arch").join(invoke));
    build.file("c/platform.c").file("c/shim.c");
    build.compile("wamr");
}
