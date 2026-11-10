// expect: operator `+` cannot be applied to `int?`
fn f(int a, int? b) int {
  return a + b
}
