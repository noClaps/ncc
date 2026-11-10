// expect: `uint N` of `f` is a constant, but `int` is a type
fn f<type T, uint N>(T[N] _a) {}
f<int, int>([1])
