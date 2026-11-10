fn fibonacci(int n) int {
  mut int a = 0
  mut int b = 1

  mut int i = 0
  while i < n {
    int temp = a + b
    a = b
    b = temp
    i = i + 1
  }
  return a
}

@println(fibonacci(10))

struct Fraction {
  int numerator
  int denominator
}

enum Status {
  Pending
  Complete
  Failed
}

fn check_status(Status st) str {
  if st {
    Status.Pending -> { return "pending" }
    Status.Complete -> { return "complete" }
    Status.Failed -> { return "failed" }
  }
}

Status stat_1 = Status.Complete
@println(check_status(stat_1))

mut Fraction f = Fraction{.numerator = 1, .denominator = 10}
f.numerator = f.numerator * 2
@println(f)

int[] nums = [1, 2, 3, 4, 5]
@println(nums[$], " ", nums[$-1], " ", nums.len)

char letter = if 2 {
  1 -> { break 'A' }
  2 -> { break 'B' }
  _ -> { break 'Z' }
}
@println(letter)

fn opt_add_num(int a, int? b) int {
  int b = b else {
    return a
  }
  return a + b
}
@println(opt_add_num(1, none), " ", opt_add_num(1, 5))

fn add_throws(int a, int b) int! {
  if {
    a > 100 -> { throw "too big" }
    _ -> { return a + b }
  }
}
int s = add_throws(1, 2) catch _ { break 0 }
int s2 = add_throws(200, 2) catch err {
  @eprintln(err)
  break -1
}
@println(s, " ", s2)
@println("interp {s + 1} done")

[str]int months = ["jan": 1, "feb": 2]
@println(months["feb"])

mut int val = 0
fn increment() {
  val = val + 1
}
increment()
increment()
@println(val)

fn calculator(int a, int b, (fn(int, int) int) operation) int {
  return operation(a, b)
}
fn add = fn(int a, int b) int {
  return a + b
}
@println(calculator(3, 5, add))

test "tuple" {
  int a, str b = (1, "Hi")
  assert a == 1
  assert b == "Hi"
}
