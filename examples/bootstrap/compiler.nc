// A deliberately small NC-to-C compiler, implemented in NC.
// Input: int declarations, @println, integer arithmetic, names, parentheses.
// Errors are reported to stderr. Source is read from stdin; C goes to stdout.
extern "io.c" as io { fn read_stdin() str = "bootstrap_read_stdin" }

enum Node { Number(int) Name(str) Binary(char, Node, Node) }

char[] source = @as(char[], io.read_stdin())
mut uint cursor = 0
mut uint serial = 0
mut [str]str symbols = []

fn space() {
  while cursor < source.len {
    if source[cursor] {
      ' ', '\n', '\r', '\t' -> { cursor = cursor + 1 }
      _ -> { return }
    }
  }
}

fn digit(char ch) bool {
  byte[] bytes = @as(byte[], ch)
  int code = @as(int, bytes[0])
  return bytes.len == 1 and code >= 48 and code <= 57
}

fn letter(char ch) bool {
  byte[] bytes = @as(byte[], ch)
  int code = @as(int, bytes[0])
  return bytes.len == 1 and ((code >= 65 and code <= 90) or (code >= 97 and code <= 122) or code == 95)
}

fn word() str! {
  space()
  if cursor >= source.len or not letter(source[cursor]) {
    true -> { throw "expected identifier at character {cursor}" }
    false -> {}
  }
  mut str result = ""
  while cursor < source.len and (letter(source[cursor]) or digit(source[cursor])) {
    result = result <> @as(str, source[cursor])
    cursor = cursor + 1
  }
  return result
}

fn expect(char ch) ! {
  space()
  if cursor >= source.len or source[cursor] != ch {
    true -> { throw "expected {ch} at character {cursor}" }
    false -> { cursor = cursor + 1 }
  }
}

fn primary() Node! {
  space()
  if cursor == source.len {
    true -> { throw "expected expression at end of input" }
    false -> {}
  }
  char ch = source[cursor]
  if {
    ch == '(' -> {
      cursor = cursor + 1
      Node result = try expression(0)
      try expect(')')
      return result
    }
    ch == '-' -> {
      cursor = cursor + 1
      return Node.Binary('-', Node.Number(0), try primary())
    }
    digit(ch) -> {
      mut int number = 0
      while cursor < source.len and digit(source[cursor]) {
        byte[] bytes = @as(byte[], source[cursor])
        number = number * 10 + @as(int, bytes[0]) - 48
        cursor = cursor + 1
      }
      return Node.Number(number)
    }
    _ -> {
      str name = try word()
      if name in symbols {
        true -> { return Node.Name(symbols[name]) }
        false -> { throw "unknown name: {name}" }
      }
    }
  }
}

fn precedence(char op) int {
  return if op { '+', '-' -> { 1 } '*', '/' -> { 2 } _ -> { -1 } }
}

fn expression(int minimum) Node! {
  mut Node left = try primary()
  space()
  while cursor < source.len {
    char op = source[cursor]
    int power = precedence(op)
    if power < minimum {
      true -> { return left }
      false -> {}
    }
    cursor = cursor + 1
    Node right = try expression(power + 1)
    left = Node.Binary(op, left, right)
    space()
  }
  return left
}

fn emit(Node node) str {
  return if node {
    Node.Number(number) -> { "{number}LL" }
    Node.Name(name) -> { name }
    Node.Binary(op, left, right) -> { "({emit(left)} {op} {emit(right)})" }
  }
}

fn compile() str! {
  mut str output = "#include <stdio.h>\nint main(void) \{\n"
  space()
  while cursor < source.len {
    if source[cursor] {
      '@' -> {
        cursor = cursor + 1
        str builtin = try word()
        if builtin { "println" -> {} _ -> { throw "unsupported builtin: {builtin}" } }
        try expect('(')
        Node value = try expression(0)
        try expect(')')
        output = output <> "printf(\"%lld\\n\", (long long){emit(value)});\n"
      }
      _ -> {
        str type_name = try word()
        if type_name { "int" -> {} _ -> { throw "only int declarations are supported" } }
        str name = try word()
        try expect('=')
        Node value = try expression(0)
        str local = "local_{serial}"
        serial = serial + 1
        output = output <> "long long {local} = {emit(value)};\n"
        symbols[name] = local
      }
    }
    space()
  }
  return output <> "return 0;\n}\n"
}

@print(try compile())
