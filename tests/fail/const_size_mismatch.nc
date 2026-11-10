// expect: expected exactly 3 elements for `int[3]`, found 2
fn f<type T, uint N>(T[N + 1] _a) {}
f<int, 2>([1, 2])
