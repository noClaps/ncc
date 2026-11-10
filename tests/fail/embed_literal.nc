// expect: `@embed` paths must be plain string literals
str p = "./x"
byte[] b = @embed(p)
