// expect: `if` branches are not exhaustive; add a fallback `_ -> { ... }` branch
int n = 1
if n {
  1 -> {}
}
