// expect: expected `str`, found `C`
type C = str
fn f(str a) {
}
C c = "x"
f(c)
