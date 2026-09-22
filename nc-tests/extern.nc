extern "add.c" as add {
    fn add(str a, str b) int = "add"
}

@println(add.add(10, 20))
