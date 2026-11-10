str[] args = @args()
@println(args.len)

[str]str env = @env()
@println("PATH" in env)

str os, str cpu = @target()
@println(os == "macos" or os == "linux", " ", cpu.len > 0)
(str, str) target = @target()
@println(target[0] == os)

byte[] file = @embed("./data/hello.txt")
@println(file.len, " ", file[0])
mut byte[] copy = file
copy[0] = 69
@println(file[0], " ", copy[0])
