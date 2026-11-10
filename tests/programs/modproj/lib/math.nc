pub float PI = 3.14159265358979323

int A = 8121
int C = 28411
int M = 134456
mut int seed = 123456789
pub fn random_lcg() int {
  seed = (A * seed + C) % M
  return seed
}

@println("this should never print")

pub struct Point {
  int x
  int y
}

extern "add.c" as cadd {
  fn add(int a, int b) int = "nc_test_add"
  fn safe_div(int a, int b) int! = "nc_test_div"
}

pub fn add(int a, int b) int {
  return cadd.add(a, b)
}
pub fn div(int a, int b) int! {
  return try cadd.safe_div(a, b)
}
