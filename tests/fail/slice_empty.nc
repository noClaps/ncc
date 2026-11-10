// expect: slice `[0:-1]` starts after it ends (in instantiation `split<int, 0, 3>`)
fn split<type T, uint N, uint M>(T[N + M] arr) (T[N], T[M]) {
  return (arr[0:N - 1], arr[N:$])
}
int[0] _a, int[3] _b = split<int, 0, 3>([1, 2, 3])
