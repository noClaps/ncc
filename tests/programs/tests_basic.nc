test "shadowing" {
  int a = 10
  assert a == 10
  mut int a = 20
  a = a + 1
  assert a == 21
  str a = "hi"
  {
    str a = "annyeonghaseyo"
    assert a.len == 14
  }
  assert a == "hi"
}

test "$ indexing" {
  int[] nums = [1, 2, 3, 4, 5]
  assert nums[0] == 1
  assert nums[$] == 5
  assert nums[$-1] == 4
}

test "fixed-size arrays" {
  mut int[5] fixed_array = [1, 2, 3, 4, 5]
  fixed_array[0] = 3
  assert fixed_array[$] == 5
  assert fixed_array.len == 5
  assert fixed_array[0] == 3
}

test "fixed-size array concatenation" {
  int[2] first = [1, 2]
  int[3] second = [3, 4, 5]
  int[5] total = first <> second
  assert total == [1, 2, 3, 4, 5]
  int[] dyn = first <> second
  assert dyn == [1, 2, 3, 4, 5]
}

struct Fraction {
  int numerator
  int denominator
}

test "mutable struct" {
  mut Fraction my_frac = Fraction{.numerator = 1, .denominator = 10}
  my_frac.numerator = my_frac.numerator * 2
  assert my_frac == Fraction{.numerator = 2, .denominator = 10}
}

fn my_func(int a, int b) (int, int) {
  int c = a + b
  int d = a - b
  return c, d
}

test "function return tuple" {
  (int, int) vals = my_func(2, 4)
  assert vals == (6, -2)
}

test "tuple indexing" {
  (int, str, float) vals = (1, "hi", 2.5)
  assert vals[0] == 1
  assert vals[1] == "hi"
  assert vals[2] == 2.5
}

test "partial tuple destructuring" {
  (int, int, int) vals = (1, 2, 3)
  int a, (int, int) b = vals
  assert a == 1
  assert b == (2, 3)
}

test "destructuring optional tuple" {
  (int, int)? vals = (3, 4)
  (int, int) vals = vals else {
    throw "Cannot destructure a none tuple"
  }
  int a, int b = vals
  assert a == 3 and b == 4
}

test "checking none tuple (should fail)" {
  (int, int)? vals = none
  vals else {
    throw "Cannot destructure a none tuple"
  }
}

test "failing test" {
  assert 2 + 2 == 5
}

test "breaking for assignment" {
  mut int index = 5
  char letter = if index {
    1 -> { break 'A' }
    _ -> {
      while index < 26 {
        lbl: if index {
          5 -> { break 'E' }
          6 -> { break :lbl }
          7 -> { break }
          _ -> {}
        }
        index = index + 1
      }
      break 'Z'
    }
  }
  assert letter == 'E'
}

test "type in type[]" {
  int[] arr = [1, 2, 3, 4, 5]
  assert 5 in arr
  [char]int map = ['a': 1, 'b': 2, 'c': 3]
  assert 'a' in map
  str string = "Hello world"
  assert 'H' in string
  assert "Hello" in string
  assert not ("xyz" in string)
}

test "converting int to float" {
  int n = 5
  float f = @as(float, n)
  assert f == 5.0
}

fn opt_set_default(int? opt, int default) int {
  int _out = opt else default
  int out = opt else {
    break default
  }
  return out
}

test "optional set default" {
  int val = opt_set_default(5, 6)
  assert val == 5
  int val = opt_set_default(none, 7)
  assert val == 7
}

fn opt_throw(int? opt, str err_msg) int! {
  return opt else { throw err_msg }
}

test "optional throw error" {
  int val = opt_throw(5, "should not fail") catch err {
    throw err
  }
  assert val == 5
  _ = opt_throw(none, "should fail") catch err {
    throw err
  }
}

test "operators" {
  assert 2 ** 6 == 64
  assert 10 % 4 == 2
  assert 5.0 / 2.0 == 2.5
  assert 5 / 2 == 2
  assert 1 << 4 == 16
  assert -3 << 1 == -6
  assert 6 >> 1 == 3
  assert -3 >> 1 == -2
  assert 27 & 1 == 1
  assert 8 | 1 == 9
  assert 12 ^ 1 == 13
  assert !6 == -7
  assert [1, 2] <> [3] == [1, 2, 3]
  assert "Hello" <> " " <> "world" == "Hello world"
}

test "maps" {
  mut [str]int m = ["jan": 1, "feb": 2]
  m["mar"] = 3
  assert m.len == 3
  [str]int m2 = m <> ["apr": 4, "jan": 10]
  assert m2["jan"] == 10
  assert m["jan"] == 1
  mut int total = 0
  for k in m2 {
    total = total + m2[k]
  }
  assert total == 19
}

test "value semantics" {
  int[] a = [1, 2, 3]
  mut int[] b = a
  b[0] = 9
  assert a[0] == 1
  assert b[0] == 9
}
