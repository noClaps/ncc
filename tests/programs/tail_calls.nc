// Self calls in tail position reuse the frame, so these recurse far deeper than the stack allows.

fn count(int n, int acc) int {
  if { n == 0 -> { return acc } _ -> {} }
  return count(n - 1, acc + 1)
}
@println(count(10000000, 0))

// tail calls as the values of a returned `if`
fn gcd(int a, int b) int {
  return if {
    b == 0 -> { break a }
    _ -> { break gcd(b, a % b) }
  }
}
@println(gcd(1071, 462))

fn parity(int n) str {
  return if n {
    0 -> { break "even" }
    1 -> { break "odd" }
    _ -> { break parity(n - 2) }
  }
}
@println(parity(5000001))

// managed parameters: each step's arguments outlive the locals they are built from
fn build(int n, str acc, int[] seen) str {
  if { n == 0 -> { return "{acc.len} {seen.len}" } _ -> {} }
  str next = acc <> "x"
  int[] more = seen <> [n]
  return build(n - 1, next, more)
}
@println(build(1000, "", []))

// arguments read the parameters they replace
fn swap_down(int n, str a, str b) str {
  if { n == 0 -> { return a <> b } _ -> {} }
  return swap_down(n - 1, b, a)
}
@println(swap_down(1000001, "left", "right"))

// a void function whose last statement is the self call
mut int ticks = 0
fn tick(int n) {
  if { n == 0 -> { return } _ -> {} }
  ticks = ticks + 1
  tick(n - 1)
}
tick(3000000)
@println(ticks)

// a void function ending in an `if` with the call in one arm
fn walk(int n) {
  if {
    n > 0 -> { walk(n - 1) }
    _ -> { @println("walked") }
  }
}
walk(3000000)

// throwing functions: `return try self(...)`
fn find(int[] xs, int target, uint i) uint! {
  if { i == xs.len -> { throw "{target} not found" } _ -> {} }
  if { xs[i] == target -> { return i } _ -> {} }
  return try find(xs, target, i + 1)
}
@println(try find([4, 8, 15, 16, 23, 42], 23, 0))
uint missing = find([1, 2, 3], 7, 0) catch err {
  @println(err)
  break 0
}
@println(missing)

fn countdown(int n, int stop) int! {
  if { n == stop -> { throw "stopped at {n}" } _ -> {} }
  if { n == 0 -> { return 0 } _ -> {} }
  return try countdown(n - 1, stop)
}
@println(try countdown(5000000, -1))
_ = countdown(5000000, 1234) catch err {
  @println(err)
  break 0
}

// a call inside a loop that returns it is still a tail call
fn first_even(int n) int {
  while n < 100 {
    if { n % 2 == 0 -> { return n } _ -> {} }
    return first_even(n + 1)
  }
  return -1
}
@println(first_even(7))

// not tail calls: the result is used afterwards
fn sum(int n) int {
  if { n == 0 -> { return 0 } _ -> {} }
  return n + sum(n - 1)
}
@println(sum(1000))

// a recursive closure through its own name
fn outer(int limit) int {
  fn loop(int i, int acc) int {
    if { i > limit -> { return acc } _ -> {} }
    return loop(i + 1, acc + i)
  }
  return loop(1, 0)
}
@println(outer(2000000))
