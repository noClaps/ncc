// expect: expected `int[2]`, found `int[]`
int[3] xs = [1, 2, 3]
uint i = 1
int[2] _y = xs[i:i + 2]
