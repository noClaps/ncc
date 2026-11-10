// expect: cannot concatenate maps of different types
[str]int a = ["a": 1]
[str]float b = ["b": 1.0]
_ = a <> b
