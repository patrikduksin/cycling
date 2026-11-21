/* VANA bare-metal platform for WAMR: one thread, no OS, memory from a pool. */
#ifndef _PLATFORM_INTERNAL_H
#define _PLATFORM_INTERNAL_H
#include <stdint.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
#include <limits.h>
#include <errno.h>
#include <math.h>
#include <ctype.h>

#ifndef BH_PLATFORM_VANA
#define BH_PLATFORM_VANA
#endif

typedef uint32_t korp_tid;
typedef uint32_t korp_mutex;
typedef uint32_t korp_cond;
typedef uint32_t korp_thread;
typedef uint32_t korp_rwlock;
typedef uint32_t korp_sem;
#define OS_THREAD_MUTEX_INITIALIZER 0
#define BH_APPLET_PRESERVED_STACK_SIZE (2 * BH_KB)
#define BH_THREAD_DEFAULT_PRIORITY 5

static inline int os_getpagesize(void) { return 4096; }
typedef int os_file_handle;
typedef void *os_dir_stream;
typedef int os_raw_file_handle;
static inline os_file_handle os_get_invalid_handle(void) { return -1; }
#endif
