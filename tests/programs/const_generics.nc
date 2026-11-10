// Generic constants: `uint N` parameters, sizes computed from them, and `N` as a value

fn size<type T, uint N>(T[N] _array) uint {
  return N
}

fn join<type T, uint N, uint M>(T[N] a, T[M] b) T[N + M] {
  return a <> b
}

fn twice<uint N>(int[N] a) int[2 * N] {
  return join<int, N, N>(a, a)
}

fn offset<int K>(int x) int {
  return x + K * (K - 1)
}

fn sum<uint N>(int[N] xs) int {
  mut int total = 0
  for i in xs {
    total = total + xs[i]
  }
  return total
}

struct Grid<type T, uint W, uint H> {
  T[W * H] cells
}

fn cell<type T, uint W, uint H>(Grid<T, W, H> g, uint x, uint y) T {
  return g.cells[y * W + x]
}

enum Shape<uint N> {
  Point
  Poly(float[N])
}

int[3] xs = [1, 2, 3]
@println("{size<int, 3>(xs)} {size<str, 0>([])}")
int[6] ys = twice<3>(xs)
@println("{ys} {sum<6>(ys)}")
int[5] zs = join<int, 2, 3>([1, 2], [3, 4, 5])
@println("{zs}")
@println("{offset<-2>(1)} {offset<3>(1)}")
Grid<str, 2, 3> g = Grid<str, 2, 3>{.cells = ["a", "b", "c", "d", "e", "f"]}
@println("{cell<str, 2, 3>(g, 1, 2)}")
Shape<2> s = Shape<2>.Poly([1.5, 2.5])
if s {
  Shape<2>.Poly([a, b]) -> { @println("{a + b}") }
  _ -> {}
}
