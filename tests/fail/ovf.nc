// expect: this expression always panics: integer overflow
fn f(int a) int {
  return a * a
}
@println(f(9223372036))
@println(f(9223372037))
