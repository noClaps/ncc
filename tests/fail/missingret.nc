// expect: function `f` does not return a value on every path
fn f(int a) int {
  if a {
    1 -> { return 1 }
    _ -> {}
  }
}
