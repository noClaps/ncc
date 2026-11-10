// expect: this expression always panics: negative value cannot be converted to uint
fn f(int i) uint {
  return @as(uint, i)
}
@println(f(-1))
