// expect: `-1` is out of range for `uint N` of `f`
fn f<uint N>() {}
f<-1>()
