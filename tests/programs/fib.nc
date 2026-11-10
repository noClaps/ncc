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
