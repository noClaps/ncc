// expect: this expression always panics: index out of bounds
fn at(int[] a, uint i) int {
  return a[i]
}
@println(at([1, 2], 5u))
