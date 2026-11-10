// expect-exit: 1
// expect-stderr: error: boom
fn f() int! {
  @println("running")
  throw "boom"
}
int x = try f()
