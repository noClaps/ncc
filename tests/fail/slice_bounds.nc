// expect: slice `[2:5]` is out of bounds for `int[3]`
int[3] xs = [1, 2, 3]
_ = xs[2:5]
