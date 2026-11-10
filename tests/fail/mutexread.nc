// expect: `x` is a mutex; access it inside `lock x { ... }`
mutex int x = 1
int y = x + 1
