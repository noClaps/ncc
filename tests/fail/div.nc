// expect: this expression always panics: division by zero
fn d(int a, int b) int {
  return a / b
}
@println(d(1, 0))
