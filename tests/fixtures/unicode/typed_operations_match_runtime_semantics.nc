
enum Choice { Number(int) Empty }
fn choose(Choice c) int { if c { Choice.Number(n) -> { return n } Choice.Empty -> { return 0 } } }
fn fallback(int? n) int { return n else { break 7 } }
fn branch(bool n) int { return if n { true -> { break 1 } false -> { break 2 } } }
fn length(str s) uint { return s.len }
fn first(str s) char { return s[0] }
fn equal([float]int a, [float]int b) bool { return a == b }
fn bytes(uint n) byte[] { return @as(byte[], n) }
fn shift(byte n) byte { return n << 2 }
fn bits(uint n) uint { return !n }
fn minimum() int { return -9223372036854775808 }
fn nominal_power(int n) int { return 1 ** n }
@println(choose(Choice.Number(42)))
@println(fallback(none))
@println(fallback(5))
@println(branch(false))
@println(length("ö🍪"))
@println(first("🍪"))
@println(equal([0.0:1,2.0:2], [2.0:2,-0.0:1]))
@println(bytes(258))
@println(shift(3))
@println(bits(0))
@println(minimum())
@println(nominal_power(9223372036854775807))
