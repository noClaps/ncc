// expect-exit: 101
// expect-stderr: panic: integer overflow
fn square(int a) int {
  @print("")
  return a * a
}
@println(square(3))
@println(square(9223372037))
