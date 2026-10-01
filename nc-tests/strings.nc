mut str[] buf = []
mut int i = 0
while i < 1024 {
    buf = buf <> [""]
    i = i + 1
}

fn from_int(int n) {
    mut int i = 0
    while i < n {
        buf[i & 1023] = "{i}"
        i = i + 1
    }
}

from_int(100000000)
@println(buf)
