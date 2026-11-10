/* NC runtime: types shared by generated code and extern C files.
 * Define NC_RUNTIME_IMPL in exactly one translation unit to include the implementation. */
#ifndef NC_RUNTIME_H
#define NC_RUNTIME_H

#include <math.h>
#include <pthread.h>
#include <setjmp.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdckdint.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef uint8_t nc_void;

/* ------------------------------------------------------------------ strings */

/* UTF-8 string. `rc < 0` marks an immortal (static) string. */
typedef struct nc_str {
  _Atomic int64_t rc;
  int64_t blen; /* bytes */
  int64_t clen; /* unicode scalar values */
  const char *data;
} nc_str;

/* String builder used for conversions to `str`. */
typedef struct nc_sb {
  char *buf;
  int64_t len, cap;
} nc_sb;

/* Type information for values stored in generic containers. */
typedef struct nc_ti {
  size_t size;
  void (*retain)(void *);
  void (*release)(void *);
  bool (*eq)(const void *, const void *);
  uint64_t (*hash)(const void *);
  void (*str)(nc_sb *, const void *);
} nc_ti;

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

/* Heap cell: captured mutable variables, boxed optionals. */
typedef struct nc_cell {
  _Atomic int64_t rc;
  const nc_ti *ti;
  alignas(max_align_t) unsigned char val[];
} nc_cell;

/* Mutex-protected value. */
typedef struct nc_mutex {
  _Atomic int64_t rc;
  pthread_mutex_t mu;
  const nc_ti *ti;
  alignas(max_align_t) unsigned char val[];
} nc_mutex;

/* Closure environment header; generated environments embed it first. */
typedef struct nc_env {
  _Atomic int64_t rc;
  void (*drop)(struct nc_env *);
} nc_env;

typedef struct nc_fn {
  void *fp;
  nc_env *env;
} nc_fn;

/* Future: generated per-call-site structs embed this header first. */
typedef struct nc_fut {
  _Atomic int64_t rc;
  pthread_mutex_t mu;
  pthread_cond_t cv;
  int state; /* 0 pending, 1 running, 2 done */
  void (*run)(struct nc_fut *);
  void (*drop)(struct nc_fut *);
  void *result;
  struct nc_fut *next;
} nc_fut;

/* Error unions returned by extern functions use generated structs of the form
 * `struct { nc_str *err; T val; }`; `err == NULL` means success. */

_Noreturn void nc_panic(const char *msg, const char *loc);
nc_str *nc_str_new(const char *bytes, int64_t blen);
nc_str *nc_str_from_cstr(const char *s);
void nc_str_retain(nc_str *s);
void nc_str_release(nc_str *s);
nc_arr *nc_arr_new(const nc_ti *ti, int64_t cap);
void nc_arr_push(nc_arr **a, const void *elem);
void nc_arr_retain(nc_arr *a);
void nc_arr_release(nc_arr *a);

#ifdef NC_RUNTIME_IMPL

/* ------------------------------------------------------------------ panics */

/* Set by the test runner: called before a panic or failed assert aborts the process, so the
 * runner can record the failure and move on to the next test. */
static void (*nc_fail_hook)(bool is_assert, const char *msg, const char *loc) = NULL;

_Noreturn void nc_panic(const char *msg, const char *loc) {
  if (nc_fail_hook) nc_fail_hook(false, msg, loc);
  fflush(stdout);
  fprintf(stderr, "panic: %s (%s)\n", msg, loc);
  fflush(stderr);
  exit(101);
}

static _Noreturn void nc_assert_fail(const char *loc) {
  if (nc_fail_hook) nc_fail_hook(true, "assertion failed", loc);
  nc_panic("assertion failed", loc);
}

/* ------------------------------------------------------------------ test runner */

static jmp_buf *nc_test_jmp = NULL;
static pthread_t nc_main_thread;
static char nc_test_msg[1024];

/* Failures on the main thread jump back to the test runner; other threads abort. */
static void nc_test_fail(bool is_assert, const char *msg, const char *loc) {
  if (!nc_test_jmp || !pthread_equal(pthread_self(), nc_main_thread)) return;
  if (is_assert)
    snprintf(nc_test_msg, sizeof nc_test_msg, "assertion failed at %s", loc);
  else
    snprintf(nc_test_msg, sizeof nc_test_msg, "panic: %s (%s)", msg, loc);
  longjmp(*nc_test_jmp, 1);
}

static void nc_test_runner_init(void) {
  nc_main_thread = pthread_self();
  nc_fail_hook = nc_test_fail;
}

static void *nc_alloc(size_t n) {
  void *p = malloc(n ? n : 1);
  if (!p) {
    fputs("panic: out of memory\n", stderr);
    exit(101);
  }
  return p;
}

static void *nc_zalloc(size_t n) {
  void *p = calloc(1, n ? n : 1);
  if (!p) {
    fputs("panic: out of memory\n", stderr);
    exit(101);
  }
  return p;
}

static inline void nc_inc(_Atomic int64_t *rc) {
  if (atomic_load_explicit(rc, memory_order_relaxed) >= 0) atomic_fetch_add_explicit(rc, 1, memory_order_relaxed);
}
/* returns true when the object must be freed */
static inline bool nc_dec(_Atomic int64_t *rc) {
  if (atomic_load_explicit(rc, memory_order_relaxed) < 0) return false;
  return atomic_fetch_sub_explicit(rc, 1, memory_order_acq_rel) == 1;
}
static inline bool nc_unique(_Atomic int64_t *rc) { return atomic_load_explicit(rc, memory_order_acquire) == 1; }

/* ------------------------------------------------------------------ checked integer arithmetic */

static inline int64_t nc_add_i64(int64_t a, int64_t b, const char *loc) {
  int64_t r;
  if (ckd_add(&r, a, b)) nc_panic("integer overflow", loc);
  return r;
}
static inline int64_t nc_sub_i64(int64_t a, int64_t b, const char *loc) {
  int64_t r;
  if (ckd_sub(&r, a, b)) nc_panic("integer overflow", loc);
  return r;
}
static inline int64_t nc_mul_i64(int64_t a, int64_t b, const char *loc) {
  int64_t r;
  if (ckd_mul(&r, a, b)) nc_panic("integer overflow", loc);
  return r;
}
static inline int64_t nc_pow_i64(int64_t a, int64_t b, const char *loc) {
  if (b < 0) nc_panic("negative exponent", loc);
  int64_t r = 1;
  while (b > 0) {
    if ((b & 1) && ckd_mul(&r, r, a)) nc_panic("integer overflow", loc);
    b >>= 1;
    if (b > 0 && ckd_mul(&a, a, a)) nc_panic("integer overflow", loc);
  }
  return r;
}
static inline uint64_t nc_add_u64(uint64_t a, uint64_t b, const char *loc) {
  uint64_t r;
  if (ckd_add(&r, a, b)) nc_panic("integer overflow", loc);
  return r;
}
static inline uint64_t nc_sub_u64(uint64_t a, uint64_t b, const char *loc) {
  uint64_t r;
  if (ckd_sub(&r, a, b)) nc_panic("integer overflow", loc);
  return r;
}
static inline uint64_t nc_mul_u64(uint64_t a, uint64_t b, const char *loc) {
  uint64_t r;
  if (ckd_mul(&r, a, b)) nc_panic("integer overflow", loc);
  return r;
}
static inline uint64_t nc_pow_u64(uint64_t a, uint64_t b, const char *loc) {
  if (b < 0) nc_panic("negative exponent", loc);
  uint64_t r = 1;
  while (b > 0) {
    if ((b & 1) && ckd_mul(&r, r, a)) nc_panic("integer overflow", loc);
    b >>= 1;
    if (b > 0 && ckd_mul(&a, a, a)) nc_panic("integer overflow", loc);
  }
  return r;
}
static inline uint8_t nc_add_u8(uint8_t a, uint8_t b, const char *loc) {
  uint8_t r;
  if (ckd_add(&r, a, b)) nc_panic("integer overflow", loc);
  return r;
}
static inline uint8_t nc_sub_u8(uint8_t a, uint8_t b, const char *loc) {
  uint8_t r;
  if (ckd_sub(&r, a, b)) nc_panic("integer overflow", loc);
  return r;
}
static inline uint8_t nc_mul_u8(uint8_t a, uint8_t b, const char *loc) {
  uint8_t r;
  if (ckd_mul(&r, a, b)) nc_panic("integer overflow", loc);
  return r;
}
static inline uint8_t nc_pow_u8(uint8_t a, uint8_t b, const char *loc) {
  if (b < 0) nc_panic("negative exponent", loc);
  uint8_t r = 1;
  while (b > 0) {
    if ((b & 1) && ckd_mul(&r, r, a)) nc_panic("integer overflow", loc);
    b >>= 1;
    if (b > 0 && ckd_mul(&a, a, a)) nc_panic("integer overflow", loc);
  }
  return r;
}

static inline int64_t nc_div_i64(int64_t a, int64_t b, const char *loc) {
  if (b == 0) nc_panic("division by zero", loc);
  if (a == INT64_MIN && b == -1) nc_panic("integer overflow", loc);
  return a / b;
}
static inline int64_t nc_mod_i64(int64_t a, int64_t b, const char *loc) {
  if (b == 0) nc_panic("division by zero", loc);
  if (b == -1) return 0;
  return a % b;
}
static inline uint64_t nc_div_u64(uint64_t a, uint64_t b, const char *loc) {
  if (b == 0) nc_panic("division by zero", loc);
  return a / b;
}
static inline uint64_t nc_mod_u64(uint64_t a, uint64_t b, const char *loc) {
  if (b == 0) nc_panic("division by zero", loc);
  return a % b;
}
static inline uint8_t nc_div_u8(uint8_t a, uint8_t b, const char *loc) {
  if (b == 0) nc_panic("division by zero", loc);
  return (uint8_t)(a / b);
}
static inline uint8_t nc_mod_u8(uint8_t a, uint8_t b, const char *loc) {
  if (b == 0) nc_panic("division by zero", loc);
  return (uint8_t)(a % b);
}
static inline int64_t nc_neg_i64(int64_t a, const char *loc) {
  if (a == INT64_MIN) nc_panic("integer overflow", loc);
  return -a;
}
static inline int64_t nc_shl_i64(int64_t a, int64_t n, const char *loc) {
  if (n < 0 || n >= 64) nc_panic("shift amount out of range", loc);
  if (n == 63) {
    if (a == 0) return 0;
    if (a == -1) return INT64_MIN;
    nc_panic("integer overflow", loc);
  }
  int64_t r;
  if (ckd_mul(&r, a, (int64_t)1 << n)) nc_panic("integer overflow", loc);
  return r;
}
static inline int64_t nc_shr_i64(int64_t a, int64_t n, const char *loc) {
  if (n < 0 || n >= 64) nc_panic("shift amount out of range", loc);
  return a >= 0 ? a >> n : ~(~a >> n);
}
static inline uint64_t nc_shl_u64(uint64_t a, uint64_t n, const char *loc) {
  if (n >= 64) nc_panic("shift amount out of range", loc);
  if (n > 0 && (a >> (64 - n)) != 0) nc_panic("integer overflow", loc);
  return a << n;
}
static inline uint64_t nc_shr_u64(uint64_t a, uint64_t n, const char *loc) {
  if (n >= 64) nc_panic("shift amount out of range", loc);
  return a >> n;
}
static inline uint8_t nc_shl_u8(uint8_t a, uint8_t n, const char *loc) {
  if (n >= 8) nc_panic("shift amount out of range", loc);
  if (((unsigned)a << n) > 0xFF) nc_panic("integer overflow", loc);
  return (uint8_t)(a << n);
}
static inline uint8_t nc_shr_u8(uint8_t a, uint8_t n, const char *loc) {
  if (n >= 8) nc_panic("shift amount out of range", loc);
  return (uint8_t)(a >> n);
}

/* conversions */
static inline uint64_t nc_i64_to_u64(int64_t v, const char *loc) {
  if (v < 0) nc_panic("negative value cannot be converted to uint", loc);
  return (uint64_t)v;
}
static inline int64_t nc_u64_to_i64(uint64_t v, const char *loc) {
  if (v > (uint64_t)INT64_MAX) nc_panic("value out of range for int", loc);
  return (int64_t)v;
}
static inline int64_t nc_f64_to_i64(double v, const char *loc) {
  if (!(v > -9223372036854775809.0 && v < 9223372036854775808.0)) nc_panic("float value out of range for int", loc);
  return (int64_t)v;
}
static inline uint64_t nc_f64_to_u64(double v, const char *loc) {
  if (!(v > -1.0 && v < 18446744073709551616.0)) nc_panic("float value out of range for uint", loc);
  return (uint64_t)v;
}
static inline uint64_t nc_index(int64_t i, const char *loc) {
  if (i < 0) nc_panic("negative index", loc);
  return (uint64_t)i;
}

/* ------------------------------------------------------------------ utf-8 */

static int nc_utf8_encode(uint32_t c, char *out) {
  if (c < 0x80) {
    out[0] = (char)c;
    return 1;
  }
  if (c < 0x800) {
    out[0] = (char)(0xC0 | (c >> 6));
    out[1] = (char)(0x80 | (c & 0x3F));
    return 2;
  }
  if (c < 0x10000) {
    out[0] = (char)(0xE0 | (c >> 12));
    out[1] = (char)(0x80 | ((c >> 6) & 0x3F));
    out[2] = (char)(0x80 | (c & 0x3F));
    return 3;
  }
  out[0] = (char)(0xF0 | (c >> 18));
  out[1] = (char)(0x80 | ((c >> 12) & 0x3F));
  out[2] = (char)(0x80 | ((c >> 6) & 0x3F));
  out[3] = (char)(0x80 | (c & 0x3F));
  return 4;
}

/* decode one scalar at `p`, returning its length */
static int nc_utf8_decode(const unsigned char *p, int64_t avail, uint32_t *out) {
  unsigned char b = p[0];
  if (b < 0x80) {
    *out = b;
    return 1;
  }
  int n = (b >= 0xF0) ? 4 : (b >= 0xE0) ? 3 : (b >= 0xC0) ? 2 : 1;
  if (n > avail || n == 1) {
    *out = 0xFFFD;
    return 1;
  }
  uint32_t c = b & (0x7F >> n);
  for (int i = 1; i < n; i++) c = (c << 6) | (p[i] & 0x3F);
  *out = c;
  return n;
}

static int64_t nc_utf8_count(const char *s, int64_t blen) {
  int64_t n = 0;
  for (int64_t i = 0; i < blen; i++)
    if (((unsigned char)s[i] & 0xC0) != 0x80) n++;
  return n;
}

/* ------------------------------------------------------------------ strings */

static nc_str nc_empty_str = {-1, 0, 0, ""};

nc_str *nc_str_new(const char *bytes, int64_t blen) {
  if (blen == 0) return &nc_empty_str;
  nc_str *s = nc_alloc(sizeof(nc_str) + (size_t)blen + 1);
  char *d = (char *)(s + 1);
  memcpy(d, bytes, (size_t)blen);
  d[blen] = 0;
  atomic_init(&s->rc, 1);
  s->blen = blen;
  s->clen = nc_utf8_count(d, blen);
  s->data = d;
  return s;
}

nc_str *nc_str_from_cstr(const char *s) { return nc_str_new(s, (int64_t)strlen(s)); }

void nc_str_retain(nc_str *s) {
  if (s) nc_inc(&s->rc);
}
void nc_str_release(nc_str *s) {
  if (s && nc_dec(&s->rc)) free(s);
}
static inline nc_str *nc_str_rt(nc_str *s) {
  nc_str_retain(s);
  return s;
}

static nc_str *nc_str_cat(nc_str *a, nc_str *b) {
  if (a->blen == 0) return nc_str_rt(b);
  if (b->blen == 0) return nc_str_rt(a);
  int64_t n = a->blen + b->blen;
  nc_str *s = nc_alloc(sizeof(nc_str) + (size_t)n + 1);
  char *d = (char *)(s + 1);
  memcpy(d, a->data, (size_t)a->blen);
  memcpy(d + a->blen, b->data, (size_t)b->blen);
  d[n] = 0;
  atomic_init(&s->rc, 1);
  s->blen = n;
  s->clen = a->clen + b->clen;
  s->data = d;
  return s;
}

static bool nc_str_eq(const nc_str *a, const nc_str *b) {
  return a == b || (a->blen == b->blen && memcmp(a->data, b->data, (size_t)a->blen) == 0);
}

static uint64_t nc_hash_bytes(const void *p, size_t n) {
  const unsigned char *s = p;
  uint64_t h = 1469598103934665603ULL;
  for (size_t i = 0; i < n; i++) {
    h ^= s[i];
    h *= 1099511628211ULL;
  }
  return h;
}
static inline uint64_t nc_hash_u64(uint64_t x) {
  x ^= x >> 33;
  x *= 0xff51afd7ed558ccdULL;
  x ^= x >> 33;
  x *= 0xc4ceb9fe1a85ec53ULL;
  x ^= x >> 33;
  return x;
}
static inline uint64_t nc_hash_mix(uint64_t h, uint64_t x) { return nc_hash_u64(h ^ (x + 0x9e3779b97f4a7c15ULL + (h << 6) + (h >> 2))); }
static uint64_t nc_str_hash(const nc_str *s) { return nc_hash_bytes(s->data, (size_t)s->blen); }

/* byte offset of the i-th scalar */
static int64_t nc_str_offset(const nc_str *s, uint64_t i) {
  if (s->blen == s->clen) return (int64_t)i;
  int64_t off = 0;
  for (uint64_t k = 0; k < i; k++) {
    off++;
    while (off < s->blen && ((unsigned char)s->data[off] & 0xC0) == 0x80) off++;
  }
  return off;
}

static uint32_t nc_str_at(const nc_str *s, uint64_t i, const char *loc) {
  if (i >= (uint64_t)s->clen) nc_panic("string index out of bounds", loc);
  int64_t off = nc_str_offset(s, i);
  uint32_t c;
  nc_utf8_decode((const unsigned char *)s->data + off, s->blen - off, &c);
  return c;
}

/* s[lo:hi], counted in scalars */
static nc_str *nc_str_slice(const nc_str *s, uint64_t lo, uint64_t hi, const char *loc) {
  if (lo > hi || hi > (uint64_t)s->clen) nc_panic("slice out of bounds", loc);
  int64_t a = nc_str_offset(s, lo), b = nc_str_offset(s, hi);
  return nc_str_new(s->data + a, b - a);
}

/* s[i] = c (copy-on-write) */
static void nc_str_set(nc_str **sp, uint64_t i, uint32_t c, const char *loc) {
  nc_str *s = *sp;
  if (i >= (uint64_t)s->clen) nc_panic("string index out of bounds", loc);
  int64_t off = nc_str_offset(s, i);
  uint32_t old;
  int olen = nc_utf8_decode((const unsigned char *)s->data + off, s->blen - off, &old);
  char enc[4];
  int nlen = nc_utf8_encode(c, enc);
  int64_t n = s->blen - olen + nlen;
  char *tmp = nc_alloc((size_t)n);
  memcpy(tmp, s->data, (size_t)off);
  memcpy(tmp + off, enc, (size_t)nlen);
  memcpy(tmp + off + nlen, s->data + off + olen, (size_t)(s->blen - off - olen));
  nc_str *r = nc_str_new(tmp, n);
  free(tmp);
  nc_str_release(s);
  *sp = r;
}

static bool nc_str_has_str(const nc_str *s, const nc_str *sub) {
  if (sub->blen == 0) return true;
  if (sub->blen > s->blen) return false;
  for (int64_t i = 0; i + sub->blen <= s->blen; i++)
    if (memcmp(s->data + i, sub->data, (size_t)sub->blen) == 0) return true;
  return false;
}
static bool nc_str_has_char(const nc_str *s, uint32_t c) {
  char enc[4];
  int n = nc_utf8_encode(c, enc);
  nc_str tmp = {-1, n, 1, enc};
  return nc_str_has_str(s, &tmp);
}

/* ------------------------------------------------------------------ string builder */

static nc_sb nc_sb_new(void) { return (nc_sb){NULL, 0, 0}; }
static void nc_sb_put(nc_sb *sb, const char *p, int64_t n) {
  if (sb->len + n > sb->cap) {
    int64_t cap = sb->cap ? sb->cap * 2 : 64;
    while (cap < sb->len + n) cap *= 2;
    sb->buf = realloc(sb->buf, (size_t)cap);
    if (!sb->buf) nc_panic("out of memory", "runtime");
    sb->cap = cap;
  }
  memcpy(sb->buf + sb->len, p, (size_t)n);
  sb->len += n;
}
static void nc_sb_cstr(nc_sb *sb, const char *s) { nc_sb_put(sb, s, (int64_t)strlen(s)); }
static void nc_sb_str(nc_sb *sb, const nc_str *s) { nc_sb_put(sb, s->data, s->blen); }
static void nc_sb_i64(nc_sb *sb, int64_t v) {
  char b[32];
  int n = snprintf(b, sizeof b, "%lld", (long long)v);
  nc_sb_put(sb, b, n);
}
static void nc_sb_u64(nc_sb *sb, uint64_t v) {
  char b[32];
  int n = snprintf(b, sizeof b, "%llu", (unsigned long long)v);
  nc_sb_put(sb, b, n);
}
static void nc_sb_f64(nc_sb *sb, double v) {
  char b[64];
  if (isnan(v)) {
    nc_sb_cstr(sb, "nan");
    return;
  }
  if (isinf(v)) {
    nc_sb_cstr(sb, v < 0 ? "-inf" : "inf");
    return;
  }
  /* shortest number of significant digits that round-trips */
  int prec = 1;
  for (; prec < 17; prec++) {
    snprintf(b, sizeof b, "%.*e", prec - 1, v);
    if (strtod(b, NULL) == v) break;
  }
  snprintf(b, sizeof b, "%.*e", prec - 1, v);
  int exp10 = atoi(strchr(b, 'e') + 1);
  int n;
  if (exp10 >= -5 && exp10 < 17) {
    int decimals = prec - 1 - exp10;
    if (decimals < 0) decimals = 0;
    n = snprintf(b, sizeof b, "%.*f", decimals, v);
    nc_sb_put(sb, b, n);
    if (decimals == 0) nc_sb_cstr(sb, ".0");
  } else {
    n = (int)strlen(b);
    nc_sb_put(sb, b, n);
  }
}
static void nc_sb_char(nc_sb *sb, uint32_t c) {
  char enc[4];
  int n = nc_utf8_encode(c, enc);
  nc_sb_put(sb, enc, n);
}
static void nc_sb_bool(nc_sb *sb, bool b) { nc_sb_cstr(sb, b ? "true" : "false"); }
static nc_str *nc_sb_finish(nc_sb *sb) {
  nc_str *s = nc_str_new(sb->buf, sb->len);
  free(sb->buf);
  sb->buf = NULL;
  sb->len = sb->cap = 0;
  return s;
}

/* Write a finished line in one call: stdio locks the stream, so concurrent prints don't mix. */
static void nc_sb_write(nc_sb *sb, bool err, bool nl) {
  if (nl) nc_sb_put(sb, "\n", 1);
  if (err) fflush(stdout);
  if (sb->len) fwrite(sb->buf, 1, (size_t)sb->len, err ? stderr : stdout);
  free(sb->buf);
  sb->buf = NULL;
  sb->len = sb->cap = 0;
}

/* ------------------------------------------------------------------ float hashing */

static uint64_t nc_hs_float(const void *a) {
  double d = *(const double *)a;
  if (d == 0) d = 0; /* -0.0 == 0.0 */
  uint64_t u;
  memcpy(&u, &d, sizeof u);
  return nc_hash_u64(u);
}

/* ------------------------------------------------------------------ arrays */

nc_arr *nc_arr_new(const nc_ti *ti, int64_t cap) {
  nc_arr *a = nc_alloc(sizeof(nc_arr));
  atomic_init(&a->rc, 1);
  a->len = 0;
  a->cap = cap;
  a->ti = ti;
  a->data = cap ? nc_alloc((size_t)cap * ti->size) : NULL;
  return a;
}
void nc_arr_retain(nc_arr *a) {
  if (a) nc_inc(&a->rc);
}
static inline nc_arr *nc_arr_rt(nc_arr *a) {
  nc_arr_retain(a);
  return a;
}
void nc_arr_release(nc_arr *a) {
  if (a && nc_dec(&a->rc)) {
    if (a->ti->release)
      for (int64_t i = 0; i < a->len; i++) a->ti->release((char *)a->data + (size_t)i * a->ti->size);
    free(a->data);
    free(a);
  }
}
static inline void *nc_arr_at(const nc_arr *a, uint64_t i, const char *loc) {
  if (i >= (uint64_t)a->len) nc_panic("index out of bounds", loc);
  return (char *)a->data + i * a->ti->size;
}
static inline uint64_t nc_dollar(int64_t len, const char *loc) {
  if (len == 0) nc_panic("`$` used on an empty sequence", loc);
  return (uint64_t)(len - 1);
}
/* a[lo:hi]: a new array holding copies of the elements */
static nc_arr *nc_arr_slice(const nc_arr *a, uint64_t lo, uint64_t hi, const char *loc) {
  if (lo > hi || hi > (uint64_t)a->len) nc_panic("slice out of bounds", loc);
  int64_t n = (int64_t)(hi - lo);
  nc_arr *b = nc_arr_new(a->ti, n);
  if (n) memcpy(b->data, (char *)a->data + lo * a->ti->size, (size_t)n * a->ti->size);
  b->len = n;
  if (a->ti->retain)
    for (int64_t i = 0; i < n; i++) a->ti->retain((char *)b->data + (size_t)i * a->ti->size);
  return b;
}
/* make `*ap` uniquely owned (copy-on-write) */
static void nc_arr_unique(nc_arr **ap) {
  nc_arr *a = *ap;
  if (nc_unique(&a->rc)) return;
  nc_arr *b = nc_arr_new(a->ti, a->len);
  memcpy(b->data, a->data, (size_t)a->len * a->ti->size);
  b->len = a->len;
  if (a->ti->retain)
    for (int64_t i = 0; i < b->len; i++) a->ti->retain((char *)b->data + (size_t)i * a->ti->size);
  nc_arr_release(a);
  *ap = b;
}
static inline void *nc_arr_slot(nc_arr **ap, uint64_t i, const char *loc) {
  if (i >= (uint64_t)(*ap)->len) nc_panic("index out of bounds", loc);
  nc_arr_unique(ap);
  return (char *)(*ap)->data + i * (*ap)->ti->size;
}
/* push takes ownership of the element bytes; array must be unique */
void nc_arr_push(nc_arr **ap, const void *elem) {
  nc_arr *a = *ap;
  if (a->len == a->cap) {
    a->cap = a->cap ? a->cap * 2 : 4;
    a->data = realloc(a->data, (size_t)a->cap * a->ti->size);
    if (!a->data) nc_panic("out of memory", "runtime");
  }
  memcpy((char *)a->data + (size_t)a->len * a->ti->size, elem, a->ti->size);
  a->len++;
}
static nc_arr *nc_arr_cat(const nc_arr *a, const nc_arr *b) {
  nc_arr *r = nc_arr_new(a->ti, a->len + b->len);
  size_t sz = a->ti->size;
  memcpy(r->data, a->data, (size_t)a->len * sz);
  memcpy((char *)r->data + (size_t)a->len * sz, b->data, (size_t)b->len * sz);
  r->len = a->len + b->len;
  if (a->ti->retain)
    for (int64_t i = 0; i < r->len; i++) a->ti->retain((char *)r->data + (size_t)i * sz);
  return r;
}
static bool nc_arr_eq(const nc_arr *a, const nc_arr *b) {
  if (a == b) return true;
  if (a->len != b->len) return false;
  size_t sz = a->ti->size;
  for (int64_t i = 0; i < a->len; i++)
    if (!a->ti->eq((char *)a->data + (size_t)i * sz, (char *)b->data + (size_t)i * sz)) return false;
  return true;
}
static uint64_t nc_arr_hash(const nc_arr *a) {
  uint64_t h = nc_hash_u64((uint64_t)a->len);
  for (int64_t i = 0; i < a->len; i++) h = nc_hash_mix(h, a->ti->hash((char *)a->data + (size_t)i * a->ti->size));
  return h;
}
static bool nc_arr_has(const nc_arr *a, const void *x) {
  for (int64_t i = 0; i < a->len; i++)
    if (a->ti->eq((char *)a->data + (size_t)i * a->ti->size, x)) return true;
  return false;
}
static void nc_arr_str(nc_sb *sb, const nc_arr *a) {
  nc_sb_put(sb, "[", 1);
  for (int64_t i = 0; i < a->len; i++) {
    if (i) nc_sb_put(sb, ", ", 2);
    a->ti->str(sb, (char *)a->data + (size_t)i * a->ti->size);
  }
  nc_sb_put(sb, "]", 1);
}

/* strings <-> arrays */
static nc_arr *nc_str_to_chars(const nc_str *s, const nc_ti *ti) {
  nc_arr *a = nc_arr_new(ti, s->clen);
  int64_t off = 0;
  while (off < s->blen) {
    uint32_t c;
    off += nc_utf8_decode((const unsigned char *)s->data + off, s->blen - off, &c);
    nc_arr_push(&a, &c);
  }
  return a;
}
static nc_arr *nc_bytes_of(const void *p, int64_t n, const nc_ti *ti) {
  nc_arr *a = nc_arr_new(ti, n);
  memcpy(a->data, p, (size_t)n);
  a->len = n;
  return a;
}
static nc_arr *nc_u64_le_bytes(uint64_t v, const nc_ti *ti) {
  unsigned char b[8];
  for (int i = 0; i < 8; i++) b[i] = (unsigned char)(v >> (8 * i));
  return nc_bytes_of(b, 8, ti);
}
static nc_arr *nc_char_bytes(uint32_t c, const nc_ti *ti) {
  char enc[4];
  int n = nc_utf8_encode(c, enc);
  return nc_bytes_of(enc, n, ti);
}

/* ------------------------------------------------------------------ maps */

static nc_map *nc_map_new(const nc_ti *kti, const nc_ti *vti, int64_t cap) {
  nc_map *m = nc_alloc(sizeof(nc_map));
  atomic_init(&m->rc, 1);
  int64_t c = 8;
  while (c < cap * 2) c *= 2;
  m->len = 0;
  m->cap = c;
  m->kti = kti;
  m->vti = vti;
  m->used = nc_zalloc((size_t)c);
  m->keys = nc_zalloc((size_t)c * kti->size);
  m->vals = nc_zalloc((size_t)c * vti->size);
  return m;
}
static inline void *nc_map_key(const nc_map *m, int64_t i) { return (char *)m->keys + (size_t)i * m->kti->size; }
static inline void *nc_map_val(const nc_map *m, int64_t i) { return (char *)m->vals + (size_t)i * m->vti->size; }
static void nc_map_retain(nc_map *m) {
  if (m) nc_inc(&m->rc);
}
static inline nc_map *nc_map_rt(nc_map *m) {
  nc_map_retain(m);
  return m;
}
static void nc_map_release(nc_map *m) {
  if (m && nc_dec(&m->rc)) {
    for (int64_t i = 0; i < m->cap; i++) {
      if (!m->used[i]) continue;
      if (m->kti->release) m->kti->release(nc_map_key(m, i));
      if (m->vti->release) m->vti->release(nc_map_val(m, i));
    }
    free(m->used);
    free(m->keys);
    free(m->vals);
    free(m);
  }
}
static int64_t nc_map_find(const nc_map *m, const void *k) {
  uint64_t h = m->kti->hash(k);
  int64_t mask = m->cap - 1;
  for (int64_t i = (int64_t)(h & (uint64_t)mask);; i = (i + 1) & mask) {
    if (!m->used[i]) return -1 - i;
    if (m->kti->eq(nc_map_key(m, i), k)) return i;
  }
}
static void *nc_map_get(const nc_map *m, const void *k, const char *loc) {
  int64_t i = nc_map_find(m, k);
  if (i < 0) nc_panic("key not found in map", loc);
  return nc_map_val(m, i);
}
static bool nc_map_has(const nc_map *m, const void *k) { return nc_map_find(m, k) >= 0; }
static void nc_map_grow(nc_map *m) {
  nc_map old = *m;
  m->cap *= 2;
  m->len = 0;
  m->used = nc_zalloc((size_t)m->cap);
  m->keys = nc_zalloc((size_t)m->cap * m->kti->size);
  m->vals = nc_zalloc((size_t)m->cap * m->vti->size);
  for (int64_t i = 0; i < old.cap; i++) {
    if (!old.used[i]) continue;
    int64_t j = -1 - nc_map_find(m, nc_map_key(&old, i));
    m->used[j] = 1;
    memcpy(nc_map_key(m, j), nc_map_key(&old, i), m->kti->size);
    memcpy(nc_map_val(m, j), nc_map_val(&old, i), m->vti->size);
    m->len++;
  }
  free(old.used);
  free(old.keys);
  free(old.vals);
}
static void nc_map_unique(nc_map **mp) {
  nc_map *m = *mp;
  if (nc_unique(&m->rc)) return;
  nc_map *n = nc_map_new(m->kti, m->vti, m->len);
  free(n->used);
  free(n->keys);
  free(n->vals);
  n->cap = m->cap;
  n->len = m->len;
  n->used = nc_alloc((size_t)m->cap);
  memcpy(n->used, m->used, (size_t)m->cap);
  n->keys = nc_alloc((size_t)m->cap * m->kti->size);
  memcpy(n->keys, m->keys, (size_t)m->cap * m->kti->size);
  n->vals = nc_alloc((size_t)m->cap * m->vti->size);
  memcpy(n->vals, m->vals, (size_t)m->cap * m->vti->size);
  for (int64_t i = 0; i < n->cap; i++) {
    if (!n->used[i]) continue;
    if (n->kti->retain) n->kti->retain(nc_map_key(n, i));
    if (n->vti->retain) n->vti->retain(nc_map_val(n, i));
  }
  nc_map_release(m);
  *mp = n;
}
/* slot for key `k` (borrowed), inserting a zeroed value if missing; makes the map unique */
static void *nc_map_slot(nc_map **mp, const void *k) {
  nc_map_unique(mp);
  nc_map *m = *mp;
  int64_t i = nc_map_find(m, k);
  if (i >= 0) return nc_map_val(m, i);
  if ((m->len + 1) * 4 > m->cap * 3) {
    nc_map_grow(m);
    i = nc_map_find(m, k);
  }
  int64_t j = -1 - i;
  m->used[j] = 1;
  memcpy(nc_map_key(m, j), k, m->kti->size);
  if (m->kti->retain) m->kti->retain(nc_map_key(m, j));
  memset(nc_map_val(m, j), 0, m->vti->size);
  m->len++;
  return nc_map_val(m, j);
}
/* set takes ownership of the value; the key is borrowed */
static void nc_map_set(nc_map **mp, const void *k, const void *v) {
  void *slot = nc_map_slot(mp, k);
  if ((*mp)->vti->release) (*mp)->vti->release(slot);
  memcpy(slot, v, (*mp)->vti->size);
}
static nc_map *nc_map_cat(nc_map *a, const nc_map *b) {
  nc_map *r = nc_map_rt(a);
  for (int64_t i = 0; i < b->cap; i++) {
    if (!b->used[i]) continue;
    void *slot = nc_map_slot(&r, nc_map_key(b, i));
    if (r->vti->release) r->vti->release(slot);
    memcpy(slot, nc_map_val(b, i), r->vti->size);
    if (r->vti->retain) r->vti->retain(slot);
  }
  return r;
}
static nc_arr *nc_map_keys(const nc_map *m, const nc_ti *kti) {
  nc_arr *a = nc_arr_new(kti, m->len);
  for (int64_t i = 0; i < m->cap; i++) {
    if (!m->used[i]) continue;
    if (kti->retain) kti->retain(nc_map_key(m, i));
    nc_arr_push(&a, nc_map_key(m, i));
  }
  return a;
}
static bool nc_map_eq(const nc_map *a, const nc_map *b) {
  if (a == b) return true;
  if (a->len != b->len) return false;
  for (int64_t i = 0; i < a->cap; i++) {
    if (!a->used[i]) continue;
    int64_t j = nc_map_find(b, nc_map_key(a, i));
    if (j < 0 || !a->vti->eq(nc_map_val(a, i), nc_map_val(b, j))) return false;
  }
  return true;
}
static uint64_t nc_map_hash(const nc_map *m) {
  uint64_t h = nc_hash_u64((uint64_t)m->len);
  for (int64_t i = 0; i < m->cap; i++) {
    if (!m->used[i]) continue;
    h ^= nc_hash_mix(m->kti->hash(nc_map_key(m, i)), m->vti->hash(nc_map_val(m, i)));
  }
  return h;
}
static void nc_map_str(nc_sb *sb, const nc_map *m) {
  nc_sb_put(sb, "[", 1);
  bool first = true;
  for (int64_t i = 0; i < m->cap; i++) {
    if (!m->used[i]) continue;
    if (!first) nc_sb_put(sb, ", ", 2);
    first = false;
    m->kti->str(sb, nc_map_key(m, i));
    nc_sb_put(sb, ": ", 2);
    m->vti->str(sb, nc_map_val(m, i));
  }
  nc_sb_put(sb, "]", 1);
}

/* ------------------------------------------------------------------ cells and mutexes */

static nc_cell *nc_cell_new(const nc_ti *ti) {
  nc_cell *c = nc_zalloc(sizeof(nc_cell) + ti->size);
  atomic_init(&c->rc, 1);
  c->ti = ti;
  return c;
}
static void nc_cell_retain(nc_cell *c) {
  if (c) nc_inc(&c->rc);
}
static inline nc_cell *nc_cell_rt(nc_cell *c) {
  nc_cell_retain(c);
  return c;
}
static void nc_cell_release(nc_cell *c) {
  if (c && nc_dec(&c->rc)) {
    if (c->ti->release) c->ti->release(c->val);
    free(c);
  }
}

static nc_mutex *nc_mutex_new(const nc_ti *ti) {
  nc_mutex *m = nc_zalloc(sizeof(nc_mutex) + ti->size);
  atomic_init(&m->rc, 1);
  pthread_mutex_init(&m->mu, NULL);
  m->ti = ti;
  return m;
}
static inline nc_mutex *nc_mutex_rt(nc_mutex *m) {
  if (m) nc_inc(&m->rc);
  return m;
}
static void nc_mutex_release(nc_mutex *m) {
  if (m && nc_dec(&m->rc)) {
    if (m->ti->release) m->ti->release(m->val);
    pthread_mutex_destroy(&m->mu);
    free(m);
  }
}

/* ------------------------------------------------------------------ closures */

static inline nc_env *nc_env_rt(nc_env *e) {
  if (e) nc_inc(&e->rc);
  return e;
}
static inline void nc_env_release(nc_env *e) {
  if (e && nc_dec(&e->rc)) {
    e->drop(e);
    free(e);
  }
}
static inline nc_fn nc_fn_rt(nc_fn f) {
  nc_env_rt(f.env);
  return f;
}
static inline void nc_fn_release(nc_fn f) { nc_env_release(f.env); }

/* ------------------------------------------------------------------ futures and the worker pool */

static pthread_mutex_t nc_pool_mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t nc_pool_cv = PTHREAD_COND_INITIALIZER;
static nc_fut *nc_pool_head = NULL, *nc_pool_tail = NULL;
static bool nc_pool_started = false;

static void nc_fut_release(nc_fut *f) {
  if (f && nc_dec(&f->rc)) {
    f->drop(f);
    pthread_mutex_destroy(&f->mu);
    pthread_cond_destroy(&f->cv);
    free(f);
  }
}
static inline nc_fut *nc_fut_rt(nc_fut *f) {
  if (f) nc_inc(&f->rc);
  return f;
}

/* claim and run a future on the current thread if nobody started it yet */
static void nc_fut_try_run(nc_fut *f) {
  pthread_mutex_lock(&f->mu);
  if (f->state != 0) {
    pthread_mutex_unlock(&f->mu);
    return;
  }
  f->state = 1;
  pthread_mutex_unlock(&f->mu);
  f->run(f);
  pthread_mutex_lock(&f->mu);
  f->state = 2;
  pthread_cond_broadcast(&f->cv);
  pthread_mutex_unlock(&f->mu);
}

static void *nc_worker(void *arg) {
  (void)arg;
  for (;;) {
    pthread_mutex_lock(&nc_pool_mu);
    while (!nc_pool_head) pthread_cond_wait(&nc_pool_cv, &nc_pool_mu);
    nc_fut *f = nc_pool_head;
    nc_pool_head = f->next;
    if (!nc_pool_head) nc_pool_tail = NULL;
    pthread_mutex_unlock(&nc_pool_mu);
    nc_fut_try_run(f);
    nc_fut_release(f);
  }
  return NULL;
}

static void nc_pool_start(void) {
  long n = 4;
#ifdef _SC_NPROCESSORS_ONLN
  extern long sysconf(int);
  n = sysconf(_SC_NPROCESSORS_ONLN);
#endif
  if (n < 1) n = 1;
  for (long i = 0; i < n; i++) {
    pthread_t t;
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, 64u << 20);
    pthread_create(&t, &attr, nc_worker, NULL);
    pthread_detach(t);
    pthread_attr_destroy(&attr);
  }
}

static void nc_fut_init(nc_fut *f, void (*run)(nc_fut *), void (*drop)(nc_fut *), void *result) {
  atomic_init(&f->rc, 1);
  pthread_mutex_init(&f->mu, NULL);
  pthread_cond_init(&f->cv, NULL);
  f->state = 0;
  f->run = run;
  f->drop = drop;
  f->result = result;
  f->next = NULL;
}

static void nc_fut_submit(nc_fut *f) {
  nc_fut_rt(f); /* reference held by the queue */
  pthread_mutex_lock(&nc_pool_mu);
  if (!nc_pool_started) {
    nc_pool_started = true;
    nc_pool_start();
  }
  if (nc_pool_tail)
    nc_pool_tail->next = f;
  else
    nc_pool_head = f;
  nc_pool_tail = f;
  pthread_cond_signal(&nc_pool_cv);
  pthread_mutex_unlock(&nc_pool_mu);
}

static void *nc_fut_await(nc_fut *f) {
  nc_fut_try_run(f);
  pthread_mutex_lock(&f->mu);
  while (f->state != 2) pthread_cond_wait(&f->cv, &f->mu);
  pthread_mutex_unlock(&f->mu);
  return f->result;
}

/* ------------------------------------------------------------------ process */

/* Set by `main` when the program uses `@args`. */
static int nc_argc;
static char **nc_argv;

/* `@args()`: the command line arguments, program name first. */
static nc_arr *nc_args(const nc_ti *str_ti) {
  nc_arr *a = nc_arr_new(str_ti, nc_argc);
  for (int i = 0; i < nc_argc; i++) {
    nc_str *s = nc_str_from_cstr(nc_argv[i]);
    nc_arr_push(&a, &s);
  }
  return a;
}

extern char **environ;

/* `@env()`: a snapshot of the environment variables. */
static nc_map *nc_environ(const nc_ti *str_ti) {
  nc_map *m = nc_map_new(str_ti, str_ti, 16);
  for (char **e = environ; e && *e; e++) {
    const char *eq = strchr(*e, '=');
    if (!eq) continue;
    nc_str *k = nc_str_new(*e, (int64_t)(eq - *e));
    nc_str *v = nc_str_from_cstr(eq + 1);
    nc_map_set(&m, &k, &v); /* borrows the key, takes the value */
    nc_str_release(k);
  }
  return m;
}

/* ------------------------------------------------------------------ errors */

static _Noreturn void nc_uncaught(nc_str *err) {
  fflush(stdout);
  fwrite("error: ", 1, 7, stderr);
  fwrite(err->data, 1, (size_t)err->blen, stderr);
  fputc('\n', stderr);
  fflush(stderr);
  exit(1);
}


#endif /* NC_RUNTIME_IMPL */
#endif /* NC_RUNTIME_H */
