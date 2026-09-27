extern "add.c" as add {
    fn add(int a, int b) int = "add"
}

@println(add.add(10, 20))
