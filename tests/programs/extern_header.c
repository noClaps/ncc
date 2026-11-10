/* An extern implementation that only needs `nc.h`. */
#include "nc.h"

#include <string.h>

/* `str? find(str haystack, str needle)`: the text from the first match on */
nc_opt_str ext_find(nc_str *haystack, nc_str *needle) {
  const char *at = strstr(haystack->data, needle->data);
  if (!at) return (nc_opt_str){.has = false};
  return (nc_opt_str){.has = true, .val = nc_str_from_cstr(at)};
}

/* `int! parse_digit(char c)` */
nc_err_int ext_parse_digit(uint32_t c) {
  if (c < '0' || c > '9') return (nc_err_int){.err = nc_str_from_cstr("not a digit")};
  return (nc_err_int){.val = (int64_t)(c - '0')};
}

/* `int sum(int[] xs)` */
int64_t ext_sum(nc_arr *xs) {
  int64_t total = 0;
  for (int64_t i = 0; i < xs->len; i++) total += ((int64_t *)xs->data)[i];
  return total;
}
