#include "nc_gen.h"
int64_t nc_test_add(int64_t a, int64_t b) { return a + b; }
nc_err_int nc_test_div(int64_t a, int64_t b) {
  if (b == 0) return (nc_err_int){.err = nc_str_from_cstr("division by zero (extern)")};
  return (nc_err_int){NULL, a / b};
}
