import {
  "./lib/math" as math
  ".../lib/uint" as uint
  "greet" as greet
}

@println("pi = {math.PI}")
@println("{uint.MIN} <= uint <= {uint.MAX}")
@println(math.random_lcg())
math.Point p = math.Point{.x = 1, .y = 2}
@println(p)
@println(math.add(2, 3))
@println(greet.hello("nc"))
int q = math.div(1, 0) catch e {
  @println(e)
  break 0
}
@println(q)
