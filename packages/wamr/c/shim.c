/* The part of WAMR's API whose arguments are C structs, flattened for Rust. */
#include <string.h>
#include <stddef.h>
#include "wasm_export.h"
#include "wasm.h"

int wamr_shim_init(void *pool, uint32_t size)
{
    RuntimeInitArgs args;
    memset(&args, 0, sizeof(args));
    args.mem_alloc_type = Alloc_With_Pool;
    args.mem_alloc_option.pool.heap_buf = pool;
    args.mem_alloc_option.pool.heap_size = size;
    return wasm_runtime_full_init(&args) ? 0 : -1;
}

/* WAMR may rewrite the bytes it loads, so it loads a copy from its pool and
 * keeps what it needs: the caller's bytes are free once this returns. */
void *wamr_shim_load(const uint8_t *bytes, uint32_t len, char *err, uint32_t errlen)
{
    uint8_t *copy = wasm_runtime_malloc(len ? len : 1);
    if (!copy) {
        strncpy(err, "no runtime memory for the module", errlen);
        return NULL;
    }
    memcpy(copy, bytes, len);
    LoadArgs args;
    memset(&args, 0, sizeof(args));
    args.wasm_binary_freeable = true;
    wasm_module_t module = wasm_runtime_load_ex(copy, len, &args, err, errlen);
    wasm_runtime_free(copy);
    return module;
}

/* The import at `index`: its module and name, and whether a native was
 * registered for it. -1 past the last, 0 for a function, 1 for another kind. */
int wamr_shim_import(void *module, int32_t index, const char **from, const char **name, int *linked)
{
    if (index >= wasm_runtime_get_import_count((wasm_module_t)module))
        return -1;
    wasm_import_t import;
    wasm_runtime_get_import_type((wasm_module_t)module, index, &import);
    *from = import.module_name;
    *name = import.name;
    *linked = import.linked;
    return import.kind == WASM_IMPORT_EXPORT_KIND_FUNC ? 0 : 1;
}

/* The export at `index` and whether it is a function; -1 past the last. */
int wamr_shim_export(void *module, int32_t index, const char **name)
{
    if (index >= wasm_runtime_get_export_count((wasm_module_t)module))
        return -1;
    wasm_export_t export_;
    wasm_runtime_get_export_type((wasm_module_t)module, index, &export_);
    *name = export_.name;
    return export_.kind == WASM_IMPORT_EXPORT_KIND_FUNC ? 0 : 1;
}

/* Whether the module names a start function, which instantiating it runs. */
bool wamr_shim_starts(void *module)
{
    return ((WASMModule *)module)->start_function != (uint32)-1;
}

/* Stop the running call: a native found the guest at fault. */
void wamr_shim_trap(void *env, const char *why)
{
    wasm_runtime_set_exception(wasm_runtime_get_module_inst((wasm_exec_env_t)env), why);
}

void *wamr_shim_instantiate(void *module, uint32_t stack, uint32_t pages, char *err, uint32_t errlen)
{
    InstantiationArgs args;
    memset(&args, 0, sizeof(args));
    args.default_stack_size = stack;
    args.max_memory_pages = pages;
    return wasm_runtime_instantiate_ex((wasm_module_t)module, &args, err, errlen);
}

/* True on success; on a trap, its message is copied to `err` and cleared. */
bool wamr_shim_call(void *env, void *func, uint32_t argc, uint32_t *argv, int limit, char *err, uint32_t errlen)
{
    wasm_runtime_set_instruction_count_limit((wasm_exec_env_t)env, limit);
    if (wasm_runtime_call_wasm((wasm_exec_env_t)env, (wasm_function_inst_t)func, argc, argv))
        return true;
    wasm_module_inst_t inst = wasm_runtime_get_module_inst((wasm_exec_env_t)env);
    const char *exception = wasm_runtime_get_exception(inst);
    strncpy(err, exception ? exception : "trap", errlen);
    if (errlen)
        err[errlen - 1] = '\0';
    wasm_runtime_clear_exception(inst);
    return false;
}

uint32_t wamr_shim_pool_used(void)
{
    mem_alloc_info_t info;
    if (!wasm_runtime_get_mem_alloc_info(&info))
        return 0;
    return info.total_size - info.total_free_size;
}
