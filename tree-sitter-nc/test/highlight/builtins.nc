@println("hello")
// <- function.builtin
// ^^^^^^ function.builtin
@print(@as(str, 1))
// <- function.builtin
//     ^^^ function.builtin
@eprintln(@args(), @env(), @target())
// <- function.builtin
//         ^^^^^ function.builtin
byte[] data = @embed("data.bin")
//            ^^^^^^ function.builtin
str escaped = "\u{1F36A}"
//             ^^^^^^^^^ string.escape
