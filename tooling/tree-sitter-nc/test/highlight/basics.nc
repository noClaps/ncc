pub fn add(int a, int b) int {
// <- keyword.modifier
//  ^ keyword.function
//     ^ function
//         ^ type.builtin
//             ^ variable.parameter
  return a + b
  // <- keyword.function
  //       ^ operator
}

mut str name = "nc {1 + 2}"
// <- keyword.modifier
//              ^ string
//                  ^ number

Status s = Status.Done
// <- type
//                 ^ property

@println(add(1, 2), true, none)
// <- function.builtin
//       ^ function.call
//                  ^ boolean
//                        ^ constant.builtin
