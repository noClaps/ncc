// expect: expected `type` or a constant's type (`int`, `uint` or `byte`) before a generic parameter, found `unit`
fn f<type T, unit N>(T[N] _a) {}
