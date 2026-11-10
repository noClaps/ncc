fn slow(int n) int {
  mut int s = 0
  mut int i = 0
  while i < n {
    s = s + i
    i = i + 1
  }
  return s
}

fn fails(int n) int! {
  if {
    n > 5 -> { throw "too big: {n}" }
    _ -> { return n }
  }
}

fut int a = async slow(1000)
fut int b = async slow(2000)
@println(await a + await b)

fut int! f = async fails(10)
int v = await f catch err {
  @println("caught ", err)
  break -1
}
@println(v)

test "async with mutex" {
  mutex int[] numbers = [1, 2, 3, 4, 5]

  fn add_1() bool {
    lock numbers {
      for i in numbers {
        numbers[i] = numbers[i] + 1
      }
      return true
    }
  }

  fn add_2() bool {
    lock numbers {
      for i in numbers {
        numbers[i] = numbers[i] + 2
      }
    }
    return true
  }

  fut bool first = async add_1()
  fut bool second = async add_2()

  if (await first, await second) {
    (true, true) -> {}
    _ -> { throw "Something went wrong" }
  }

  lock numbers {
    assert numbers == [4, 5, 6, 7, 8]
  }
}

test "many futures" {
  mutex int total = 0
  fn work(int k) int {
    lock total {
      total = total + k
    }
    return k
  }
  fut int a = async work(1)
  fut int b = async work(2)
  fut int c = async work(3)
  int s = await a + await b + await c
  assert s == 6
  lock total {
    assert total == 6
  }
}
