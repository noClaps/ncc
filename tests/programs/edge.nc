struct Node {
  int val
  Node? next
}
Node list = Node{.val = 1, .next = Node{.val = 2, .next = none}}
fn sum(Node n) int {
  Node? nx = n.next
  int rest = if nx {
    none -> { break 0 }
    _ -> {
      Node m = nx else { return n.val }
      break sum(m)
    }
  }
  return n.val + rest
}
@println(sum(list))
@println(list)

int?? nested = none
int?? nested2 = 5
@println(nested, " ", nested2)

fn outer() int {
  mut int x = 1
  fn mid() int {
    fn inner() {
      x = x * 10
    }
    inner()
    inner()
    return x
  }
  _ = mid()
  return x
}
@println(outer())

test "recursive nested" {
  fn fact(int n) int {
    return if n {
      0 -> { break 1 }
      _ -> { break n * fact(n - 1) }
    }
  }
  assert fact(5) == 120
}

mut [str]int m = ["a": 1, "b": 2]
for k in m {
  m[k <> "!"] = m[k] * 10
}
@println(m.len)

mut int w = 0
mut int odd = 0
while w < 10 {
  w = w + 1
  if w % 2 {
    0 -> { continue }
    _ -> {}
  }
  odd = odd + w
}
@println(odd)

type Ids = int[]
Ids ids = [1, 2, 3]
@println(ids, " ", @as(int[], ids).len)

fn parse(str s) int! {
  if s {
    "one" -> { return 1 }
    "two" -> { return 2 }
    _ -> { throw "unknown number: {s}" }
  }
}
fn total(str[] words) int! {
  mut int t = 0
  for i in words {
    t = t + try parse(words[i])
  }
  return t
}
int t1 = total(["one", "two", "two"]) catch _ { break -1 }
int t2 = total(["one", "six"]) catch e {
  @println(e)
  break -1
}
@println(t1, " ", t2)
