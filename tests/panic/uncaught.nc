// expect-exit: 1
// expect-stderr: error: boom
fn f() int! {
  @println("running")
  throw "boom"
}
int _x = try f()
