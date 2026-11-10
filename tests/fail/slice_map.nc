// expect: cannot slice `[str]int`; only arrays and strings can be sliced
[str]int m = ["a": 1]
_ = m[0:1]
