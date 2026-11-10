// expect: `N` is a constant, not a type
fn f<uint N>() N { return 1 }
_ = f<2>()
