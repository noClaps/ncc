enum CellState {
  Alive
  Dead
}

fn step(CellState[][] board) CellState[][] {
  mut CellState[][] next = board
  for row in board {
    for col in board[row] {
      uint last = board[row].len - 1
      CellState north = if row {
        0 -> { break board[$][col] }
        _ -> { break board[row - 1][col] }
      }
      CellState northeast = if (row, col) {
        (0, last) -> { break board[$][0] }
        (0, _) -> { break board[$][col + 1] }
        (_, last) -> { break board[row - 1][0] }
        _ -> { break board[row - 1][col + 1] }
      }
      mut int alive = 0
      if north {
        CellState.Alive -> { alive = alive + 1 }
        _ -> {}
      }
      if northeast {
        CellState.Alive -> { alive = alive + 1 }
        _ -> {}
      }
      if {
        alive <= 0 -> { next[row][col] = CellState.Dead }
        alive >= 2 -> { next[row][col] = CellState.Alive }
        _ -> {}
      }
    }
  }
  return next
}

CellState[][] board0 = [[CellState.Alive, CellState.Dead], [CellState.Dead, CellState.Alive]]
@println(step(board0))

fn some_function() ! {
  @println("in some_function")
}
void! val = some_function()
val catch err {
  throw err
}
fut void! fv = async some_function()
await fv catch err {
  throw err
}

struct Calculator {
  (fn(int, int) int) add
  (fn(int, int) int) multiply
}
Calculator calc = Calculator{.add = fn(int a, int b) int { return a + b }, .multiply = fn(int a, int b) int { return a * b }}
@println(calc.add(2, 3), " ", calc.multiply(2, 3))

fn get_len_of_first<type T>(T[] arr) uint {
  return arr[0].len
}
@println(get_len_of_first<str>(["hello", "world"]))
@println(get_len_of_first<int[]>([[1, 2, 3], [4, 5]]))
@println(get_len_of_first<[char]int>([['a': 1, 'b': 2], ['c': 3]]))
struct Rectangle {
  uint len
  uint wid
}
@println(get_len_of_first<Rectangle>([Rectangle{.len = 3, .wid = 4}]))

float pi = 3.14
if pi {
  3.14 -> { @println("Pi is (approximately) correct") }
  _ -> { @println("Pi is probably wrong!") }
}
byte bb = 0xFF
if bb {
  0x00 -> { @println("00") }
  0xFF -> { @println("FF") }
  _ -> { @println(bb) }
}
int[5] nums5 = [1, 2, 3, 4, 7]
if nums5 {
  [0, 1, 2, 3, 4] -> { @println("Starts with 0") }
  [1, 2, 3, 4, x] -> { @println("Starts with 1, 2, 3, 4; ends with {x}") }
  [a, b, c, d, e] -> { @println("Reversed: ", [e, d, c, b, a]) }
}

mutex int[] numbers = [1, 2, 3, 4, 5]
fn add_1(uint forbidden_len) bool {
  int forbidden_value = 4
  lbl: lock numbers {
    if numbers.len {
      forbidden_len -> { break }
      _ -> {}
    }
    for i in numbers {
      if numbers[i] {
        forbidden_value -> { break :lbl }
        _ -> { numbers[i] = numbers[i] + 1 }
      }
    }
  }
  return true
}
_ = add_1(5u)
lock numbers {
  @println(numbers)
}
_ = add_1(9u)
lock numbers {
  @println(numbers)
}

int[]? arr = none
int?[] arr2 = [none, 3]
@println(arr, " ", arr2)
@println(@as(str, Status.Done), " ", @as(int, 'x' == 'x'))
enum Status {
  Done
}
str s = "hello"
mut str t = s
t[0] = 'j'
@println(s, " ", t)
@println(@as(int, 3.9), " ", @as(uint, 7), " ", @as(float, 2u), " ", @as(char, @as(byte, 65)))
int[2] fx = [1, 2]
@println(@as(str, fx), " ", @as(int[], fx))
