int total = 1 +
  2 +
  3
fn f(int a,
     int b) int {
  return a *
    b
}
[str]int m = [
  "a": 1,
  "b": 2,
]
bool c = total > 2 and
  m.len == 2
@println(total, " ", f(2,
  3), " ", c)
