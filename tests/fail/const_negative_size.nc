// expect: array size `N - 1` is -1, which is not a valid size (in instantiation `rest<int, 0>`)
fn rest<type T, uint N>(T[N] _a) T[N - 1] { return [] }
int[0] x = []
_ = rest<int, 0>(x)
