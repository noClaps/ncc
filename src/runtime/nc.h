/* nc.h: NC's types for C code, such as the implementation files of `extern` blocks.
 *
 * NC type   C type            NC type    C type
 * void      nc_void           str        nc_str *   (UTF-8, NUL-terminated `data`)
 * bool      bool              T[], T[N]  nc_arr *   (`len` elements of `T` at `data`)
 * byte      uint8_t           [K]V       nc_map *
 * int       int64_t           T?         nc_opt_T   (`has`, `val`)
 * uint      uint64_t          T!         nc_err_T   (`err`, NULL on success, and `val`)
 * float     double            (fn...)    nc_fn      (function pointer and environment)
 * char      uint32_t          struct S   nc_S       (fields prefixed with `f_`)
 *
 * Optionals and error unions of the built-in types are defined below. For other types
 * (structs, tuples, `int[]?`, ...), include `nc_gen.h`, which ncc generates while building
 * a program that uses extern files. Strings, arrays and maps are reference counted: a value
 * returned to NC must be one the caller owns (for example from `nc_str_from_cstr`). */
#ifndef NC_RUNTIME_H
#define NC_RUNTIME_H

#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* runtime internals, only used through pointers here */
typedef struct nc_ti nc_ti;
typedef struct nc_sb nc_sb;
typedef struct nc_cell nc_cell;
typedef struct nc_mutex nc_mutex;
typedef struct nc_env nc_env;
typedef struct nc_fut nc_fut;

typedef uint8_t nc_void;

/* ------------------------------------------------------------------ strings */

/* UTF-8 string. `rc < 0` marks an immortal (static) string. */
typedef struct nc_str {
  _Atomic int64_t rc;
  int64_t blen; /* bytes */
  int64_t clen; /* unicode scalar values */
  const char *data;
} nc_str;

/* Dynamic (and fixed-size) arrays. */
typedef struct nc_arr {
  _Atomic int64_t rc;
  int64_t len, cap;
  const nc_ti *ti;
  void *data;
} nc_arr;

/* Hash maps with open addressing. */
typedef struct nc_map {
  _Atomic int64_t rc;
  int64_t len, cap;
  const nc_ti *kti, *vti;
  uint8_t *used;
  void *keys, *vals;
} nc_map;

typedef struct nc_fn {
  void *fp;
  nc_env *env;
} nc_fn;

_Noreturn void nc_panic(const char *msg, const char *loc);
nc_str *nc_str_new(const char *bytes, int64_t blen);
nc_str *nc_str_from_cstr(const char *s);
void nc_str_retain(nc_str *s);
void nc_str_release(nc_str *s);
nc_arr *nc_arr_new(const nc_ti *ti, int64_t cap);
void nc_arr_push(nc_arr **a, const void *elem);
void nc_arr_retain(nc_arr *a);
void nc_arr_release(nc_arr *a);

/* ------------------------------------------------------------------ optionals and errors */

/* `T?` is `struct { bool has; T val; }` and `T!` is `struct { nc_str *err; T val; }`, where
 * `err == NULL` means success. ncc generates these for every type a program uses (in
 * `nc_gen.h`); the ones for built-in types are defined here with the same names. */
typedef struct nc_opt_bool { bool has; bool val; } nc_opt_bool;
typedef struct nc_opt_byte { bool has; uint8_t val; } nc_opt_byte;
typedef struct nc_opt_int { bool has; int64_t val; } nc_opt_int;
typedef struct nc_opt_uint { bool has; uint64_t val; } nc_opt_uint;
typedef struct nc_opt_float { bool has; double val; } nc_opt_float;
typedef struct nc_opt_char { bool has; uint32_t val; } nc_opt_char;
typedef struct nc_opt_str { bool has; nc_str *val; } nc_opt_str;
typedef struct nc_err_void { nc_str *err; nc_void val; } nc_err_void;
typedef struct nc_err_bool { nc_str *err; bool val; } nc_err_bool;
typedef struct nc_err_byte { nc_str *err; uint8_t val; } nc_err_byte;
typedef struct nc_err_int { nc_str *err; int64_t val; } nc_err_int;
typedef struct nc_err_uint { nc_str *err; uint64_t val; } nc_err_uint;
typedef struct nc_err_float { nc_str *err; double val; } nc_err_float;
typedef struct nc_err_char { nc_str *err; uint32_t val; } nc_err_char;
typedef struct nc_err_str { nc_str *err; nc_str *val; } nc_err_str;

#endif /* NC_RUNTIME_H */
