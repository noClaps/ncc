fn fibonacci(int n) int {
    if n {
        0, 1 -> { return n }
        _ -> { return fibonacci(n-1) + fibonacci(n-2) }
    }
}
fn ufibonacci(uint n) uint {
    if n {
        0, 1 -> { return n }
        _ -> { return ufibonacci(n-1) + ufibonacci(n-2) }
    }
}
fn ffibonacci(float n) float {
    if n {
        0.0, 1.0 -> { return n }
        _ -> { return ffibonacci(n-1.0) + ffibonacci(n-2.0) }
    }
}
@println(fibonacci(92))
@println(ufibonacci(92))
@println(ffibonacci(255.0))
