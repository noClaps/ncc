// expect: futures cannot be mutable
fn f() int {
  return 1
}
mut fut int x = async f()
