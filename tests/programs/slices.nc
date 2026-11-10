// Slicing: `x[a:b]` (end exclusive)

fn drop_one<type T, uint N>(T[N] arr) T[N - 1] {
  return arr[1:N]
}

fn split<type T, uint N, uint M>(T[N + M] arr) (T[N], T[M]) {
  return (arr[0:N], arr[N:N + M])
}

// fixed-size arrays with constant bounds stay fixed-size
int[5] xs = [1, 2, 3, 4, 5]
int[4] rest = drop_one<int, 5>(xs)
int[2] front, int[3] back = split<int, 2, 3>(xs)
int[3] mid = xs[1:4]
int[2] last_two = xs[$ - 1:$ + 1]
int[0] empty = xs[2:2]
@println("{rest} {front} {back} {mid} {last_two} {empty}")

// bounds known only at run time give a dynamic array
str[] args = @args()
int[] dynamic = xs[args.len:args.len + 2]
@println("{dynamic}")

// dynamic arrays: the slice is a copy
mut str[] words = ["a", "b", "c", "d"]
str[] copy = words[1:3]
words[1] = "z"
@println("{copy} {words} {words[0:words.len]}")

// strings are sliced by character
str s = "héllo wörld"
@println("{s[0:5]}|{s[6:s.len]}|{s[0:$]}|{s[3:3]}|")
