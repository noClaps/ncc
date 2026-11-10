# Compiler

## Behavior

The compiler should compile NC to C, then hand it off to the system's C compiler to compile to the requested output format. The system's C compiler should be determined from the `CC` environment variable, falling back to the `cc` command. Use standard C23.

In debug mode, the code should be compiled as-is to C without any optimisations. The flags given to the C compiler should also support easy debugging. The C output of the NC compiler should be fairly readable for easier debugging.

In release mode, the code should be optimised heavily, with maximum constant folding and function inlining where it can be proven safe by the compiler. Everything that can be optimised and inlined, should be. For instance, this function:

```nc
fn fibonacci(int n) int {
  mut int a = 0
  mut int b = 1

  mut int i = 0
  while i < n {
    int temp = a + b
    a = b
    b = temp
    i = i + 1
  }
  return a
}

@println(fibonacci(10))
```

should be optimised to

```nc
@println(55)
```

before being compiled.

## CLI

```
Usage: ncc [init|build|run|check|test|lsp|fmt]

Commands:
  init        Initialises a project.
  build       Compiles the given file.
  run         Executes the given file.
  check       Type-checks the given file.
  test        Run tests in the given file.
  lsp         Starts the language server.
  fmt         Formats the given file.

Options:
  -h, --help  Show this help and exit.
```

```
Usage: ncc build <file>

Compiles the given file.

The output format of the file can also be detected from the extension of the
output file passed to --output.

Arguments:
  <file>                    The file to act upon.

Options:
  -r, --release             Build the file in release mode.
  -f, --format <c|asm|exe>  The format to compile to. (default: exe)
  -o, --output <out>        The file to output to. (default: filename)
  -h, --help                Show this help and exit.
```

```
Usage: ncc run <file>

Executes the given file.

This is a shorthand for building the file to an executable, running it, then
deleting the executable.

Arguments:
  <file>                    The file to act upon.

Options:
  -r, --release             Run the file in release mode.
  -h, --help                Show this help and exit.
```

```
Usage: ncc check <file>

Type-checks the given file.

Arguments:
  <file>      The file to act upon.

Options:
  -h, --help  Show this help and exit.
```

```
Usage: ncc test <file>

Run tests in the given file.

Arguments:
  <file>      The file to act upon.

Options:
  -h, --help  Show this help and exit.
```

```
Usage: ncc init

Initialises a project.

Options:
  --name <name>  Initialises the project with the given name.
                 (default: directory name)
  -h, --help     Show this help and exit.
```

```
Usage: ncc lsp

Starts the language server.

Options:
  -h, --help  Show this help and exit.
```

```
Usage: ncc fmt <file>

Formats the given file.

The file is edited in place.

Arguments:
  <file>              The file to act upon.

Options:
  --check             Checks if the file is formatted. Exits with code 1 if not
                      formatted.
  -o, --output [out]  Write the formatted source to the out file. Writes to
                      stdout if out is - or omitted.
  -h, --help          Show this help and exit.
```

## Project layout

```
.nc/
  config.toml
  deps/
.gitignore
main.nc
```
