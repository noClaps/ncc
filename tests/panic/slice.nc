// expect-exit: 101
// expect-stderr: panic: slice out of bounds
str[] args = @args()
int[] xs = [1, 2, 3]
// `args.len` is 1 at run time, so the slice runs past the end
@println(xs[args.len + 1:5])
