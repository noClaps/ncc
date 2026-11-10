// An extern C file that includes only `nc.h`

extern "./extern_header.c" as ext {
  fn find(str haystack, str needle) str? = "ext_find"
  fn parse_digit(char c) int! = "ext_parse_digit"
  fn sum(int[] xs) int = "ext_sum"
}

@println(ext.find("hello world", "wor") else "none")
@println(ext.find("hello", "x") else "none")
@println(ext.parse_digit('7') catch _e { break -1 })
@println(ext.parse_digit('x') catch e {
  @println(e)
  break -1
})
@println(ext.sum([1, 2, 3]))
