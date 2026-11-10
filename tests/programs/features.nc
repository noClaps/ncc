struct Element {
  str tag_name
  [str]str attributes
  Node[] children
}

enum Node {
  Root(Node[])
  Doctype
  Element(Element)
  Comment(str)
  Text(str)
}

fn render_el(Element el) str {
  mut str out = "<" <> el.tag_name <> ">"
  for i in el.children {
    out = out <> render(el.children[i])
  }
  return out <> "</" <> el.tag_name <> ">"
}

fn render(Node node) str {
  return if node {
    Node.Root(nodes) -> {
      mut str out = ""
      for i in nodes {
        out = out <> render(nodes[i])
      }
      break out
    }
    Node.Doctype -> { break "<!doctype html>" }
    Node.Element(el) -> { break render_el(el) }
    Node.Comment(comment) -> { break "<!--" <> comment <> "-->" }
    Node.Text(text) -> { break text }
  }
}

Node doc = Node.Root([Node.Doctype, Node.Element(Element{.tag_name = "p", .attributes = [], .children = [Node.Text("hi")]}), Node.Comment("c")])
@println(render(doc))
@println(Node.Comment("hello"))

enum Tree {
  Left(int)
  Right(Tree)
}

fn depth(Tree t) int {
  return if t {
    Tree.Left(0) -> { break 0 }
    Tree.Left(n) -> { break n }
    Tree.Right(inner) -> { break 1 + depth(inner) }
  }
}
@println(depth(Tree.Right(Tree.Right(Tree.Left(5)))))

struct Matrix<type T> {
  uint rows
  uint cols
  T[] data
}
Matrix<int> mat = Matrix<int>{.rows = 2, .cols = 2, .data = [1, 0, 0, 1]}
@println(mat)

fn map<type T, type U>(T[] arr, (fn(T) U) apply) U[] {
  mut U[] new_arr = []
  for i in arr {
    new_arr = new_arr <> [apply(arr[i])]
  }
  return new_arr
}
str[] arr2 = map<int, str>([1, 2, 3], fn(int n) str { return "{n}" })
@println(arr2)

struct Vec2 {
  int x
  int y
}
fn get_x<type T>(T val) int {
  return val.x
}
@println(get_x<Vec2>(Vec2{.x = 3, .y = 4}))

enum Result<type T, type E> {
  Ok(T)
  Err(E)
}
struct Data<type T> {
  T data
}
Result<Data<str>, str> val = Result<Data<str>, str>.Ok(Data<str>{.data = "hello"})
if val {
  Result<Data<str>, str>.Ok(v) -> { @println("{v.data}") }
  Result<Data<str>, str>.Err(e) -> { @eprintln("error: {e}") }
}

mut int counter = 0
fn increment() {
  counter = counter + 1
}
@println(counter)
increment()
@println(counter)
counter = counter + 5
@println(counter)
increment()
@println(counter)

fn make_adder(int n) (fn(int) int) {
  return fn(int x) int { return x + n }
}
(fn(int) int) add5 = make_adder(5)
@println(add5(10))

fn counter_fn() (fn() int) {
  mut int c = 0
  fn next() int {
    c = c + 1
    return c
  }
  return next
}
(fn() int) nx = counter_fn()
_ = nx()
_ = nx()
@println(nx())

str my_string = "cookie 🍪"
for i in my_string {
  @print(my_string[i], ",")
}
@println("")
@println(my_string.len)

@println("""
  Hello World
    Indented line
  Unindented line
  """)
@println("""
3 + 5 = {3 + 5}
8 + 10 = {8 + 10}
""")
@println("escaped: \{5 + 2}")

type CustomStr = str
CustomStr cs = @as(CustomStr, "abc")
str s = @as(str, cs)
CustomStr cs2 = "literal"
@println(s, " ", cs2)

@println(5.0, " ", 0.1 + 0.2, " ", 1.5, " ", 1.0 / 0.0)
@println(true, " ", @as(byte, 97), " ", 'x', " ", @as(int, true))
(str, int) data = ("Nathan", 24)
@println(data)
int? on = none
int? os = 3
@println(on, " ", os)
[str]int one = ["first": 1]
@println(one)
@println(@as(str, 42) <> "!")
byte[] bs = @as(byte[], "hé")
@println(bs)
char[] cs3 = @as(char[], "hé")
@println(cs3)

uint num = 5
if num {
  0, 1, 2, 3, 4, 5 -> { @println("Small number") }
  6, 7, 8, 9, 10 -> { @println("Medium number") }
  _ -> { @println("Large number") }
}

(str, int) person = ("Nathan", 30)
if person {
  ("Nathan", 23) -> { @println("Welcome!") }
  ("Nathan", age) -> { @println("Incorrect age: {age}") }
  (name, _) -> { @println("Incorrect name: {name}") }
}

int[] nums = [1, 2, 3, 4, 5]
if nums {
  [1, _] -> { @println("Starts with 1, length 2") }
  [1, a, b, c, d] -> { @println("last 4 values: [{a}, {b}, {c}, {d}]") }
  _ -> { @println(nums) }
}

struct Person {
  str name
  int age
}
Person p = Person{.name = "Nathan", .age = 20}
if p {
  Person{.name = "Nathan", .age = 23} -> { @println("Welcome!") }
  Person{.name = "Nathan", .age = age} -> { @println("Incorrect age: {age}") }
  Person{.name = name, .age = _} -> { @println("Incorrect name: {name}") }
}

rows: for i in nums {
  for j in nums {
    if nums[j] {
      2 -> { continue }
      3 -> { break }
      _ -> {}
    }
    if nums[i] {
      4 -> { continue :rows }
      5 -> { break :rows }
      _ -> {}
    }
    @print(nums[i], nums[j], " ")
  }
}
@println("")
