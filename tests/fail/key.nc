// expect: this expression always panics: key not found in map
fn g([str]int m) int {
  return m["x"]
}
@println(g(["y": 1]))
