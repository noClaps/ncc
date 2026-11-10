// expect: `str` has no member `x` (in instantiation `get_x<str>`)
fn get_x<type T>(T v) int {
  return v.x
}
str s = "a"
_ = get_x<str>(s)
