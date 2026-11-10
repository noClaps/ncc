// expect: cannot convert a dynamic array to a fixed-size array
int[] d = [1]
int[5] f = @as(int[5], d)
