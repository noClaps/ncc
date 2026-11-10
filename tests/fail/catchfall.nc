// expect: the `catch` block must provide a value with `break <value>` or leave the scope (`return`, `throw`, ...)
fn f() int! {
  return 1
}
int x = f() catch e {
  @println(e)
}
