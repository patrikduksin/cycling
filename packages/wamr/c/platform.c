/* WAMR's platform for VANA: one thread, no operating system, and every byte
 * from what the runtime was started with: its pool and its memory slabs. */
#include "platform_api_vmcore.h"
#include "platform_api_extension.h"
#include "wasm_export.h"

int bh_platform_init(void) { return 0; }
void bh_platform_destroy(void) {}
void *os_malloc(unsigned size) { (void)size; return NULL; }
void *os_realloc(void *ptr, unsigned size) { (void)ptr; (void)size; return NULL; }
void os_free(void *ptr) { (void)ptr; }
int os_printf(const char *format, ...) { (void)format; return 0; }
int os_vprintf(const char *format, va_list ap) { (void)format; (void)ap; return 0; }
/* Only logging and profiling read the clock: profiling is not built and
 * logging prints nowhere. */
uint64 os_time_get_boot_us(void) { return 0; }
uint64 os_time_thread_cputime_us(void) { return 0; }
korp_tid os_self_thread(void) { return 1; }
/* NULL turns the check off; Instance::new gives each environment its floor
 * with wasm_runtime_set_native_stack_boundary. */
uint8 *os_thread_get_stack_boundary(void) { return NULL; }
void os_thread_jit_write_protect_np(bool enabled) { (void)enabled; }
int os_mutex_init(korp_mutex *m) { *m = 0; return BHT_OK; }
int os_mutex_destroy(korp_mutex *m) { (void)m; return BHT_OK; }
int os_mutex_lock(korp_mutex *m) { (void)m; return BHT_OK; }
int os_mutex_unlock(korp_mutex *m) { (void)m; return BHT_OK; }

/* Linear memories come from fixed slabs, not the pool, so the pool never has
 * to find 64 KB in one piece and the slab count caps the live instances. */
#define MAX_SLABS 4
static uint8_t *slab_base;
static size_t slab_size;
static uint32_t slab_count;
static bool slab_used[MAX_SLABS];

int wamr_shim_slabs(uint8_t *base, uint32_t size, uint32_t count)
{
    if (count > MAX_SLABS)
        return -1;
    slab_base = base;
    slab_size = size;
    slab_count = count;
    return 0;
}

void *os_mmap(void *hint, size_t size, int prot, int flags, os_file_handle file)
{
    (void)hint; (void)prot; (void)flags; (void)file;
    if (size > slab_size)
        return NULL;
    for (uint32_t i = 0; i < slab_count; i++) {
        if (!slab_used[i]) {
            slab_used[i] = true;
            uint8_t *slab = slab_base + i * slab_size;
            memset(slab, 0, slab_size);
            return slab;
        }
    }
    return NULL;
}

void os_munmap(void *addr, size_t size)
{
    (void)size;
    if (addr && slab_size)
        slab_used[((uint8_t *)addr - slab_base) / slab_size] = false;
}

int os_mprotect(void *addr, size_t size, int prot) { (void)addr; (void)size; (void)prot; return 0; }
void os_dcache_flush(void) {}
void os_icache_flush(void *start, size_t len) { (void)start; (void)len; }

/* A slab is zeroed whole when taken, so growing within it needs nothing. */
void *os_mremap(void *old_addr, size_t old_size, size_t new_size)
{
    (void)old_size;
    return new_size <= slab_size ? old_addr : NULL;
}

/* Only C-API natives return traps, and wasm_c_api.c is not built: this
 * satisfies the linker. */
void wasm_trap_delete(void *trap) { (void)trap; }

#ifdef VANA_NO_LIBC
/* WAMR sorts and searches with these, and the bare-metal build has no C
 * library. */
static void swap_bytes(unsigned char *a, unsigned char *b, size_t n)
{ while (n--) { unsigned char t = *a; *a++ = *b; *b++ = t; } }

void qsort(void *base, size_t n, size_t size, int (*cmp)(const void *, const void *))
{
    unsigned char *b = base;
    for (size_t i = 1; i < n; i++)
        for (size_t j = i; j > 0 && cmp(b + (j - 1) * size, b + j * size) > 0; j--)
            swap_bytes(b + (j - 1) * size, b + j * size, size);
}

void *bsearch(const void *key, const void *base, size_t n, size_t size, int (*cmp)(const void *, const void *))
{
    const unsigned char *b = base;
    size_t lo = 0, hi = n;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        int c = cmp(key, b + mid * size);
        if (c == 0) return (void *)(b + mid * size);
        if (c < 0) hi = mid; else lo = mid + 1;
    }
    return NULL;
}
#endif
