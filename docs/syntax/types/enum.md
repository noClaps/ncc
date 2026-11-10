---
title: Enum
---

Enums allow you to have a set of named values, behaving similarly to enums in languages like TypeScript. You can define an enum using the `enum` keyword and a label, which will become the type name.

```nc
enum Status {
  Pending
  Complete
  Failed
}
```

You can access enum members with `<enum>.<member>`. You can then use this in your code:

```nc
fn check_status(Status st) str {
  if st {
    Status.Pending -> { return "pending" }
    Status.Complete -> { return "complete" }
    Status.Failed -> { return "failed" }
  }
}

fn did_fail(Status st) bool {
    return st == Status.Failed
}

Status stat_1 = Status.Complete
Status stat_2 = Status.Failed

@print(check_status(stat_1)) // complete
@print(did_fail(stat_2)) // true
```

Enums allow you to define different states of an object. For example, if you were making Conway's Game of Life, you may want to encode the cell state using an enum instead of a boolean, for clarity:

```nc
enum CellState {
  Alive
  Dead
}
```

You would then use this in your code:

```nc
mut CellState[][] board = [[...]]

for row in board {
  for col in board[row] {
    CellState north = if row {
      0 -> { board[$][col] }
      _ -> { board[row-1][col] }
    }
    CellState northeast = if (row, col) {
      (0, board[row].len) -> { board[$][0] }
      (0, _) -> { board[$][col+1] }
      (_, board[row].len) -> { board[row-1][0] }
      _ -> { board[row-1][col+1] }
    }
    CellState east = if col {
      board[row].len -> { board[row][0] }
      _ -> { board[row][col+1] }
    }
    // ...

    mut int alive_neighbors = 0
    if north {
      CellState.Alive -> { alive_neighbors = alive_neighbors + 1 }
      _ -> {}
    }
    if northeast {
      CellState.Alive -> { alive_neighbors = alive_neighbors + 1 }
      _ -> {}
    }
    // ...

    if {
      alive_neighbors <= 1 -> { board[row][col] = CellState.Dead }
      alive_neighbors >= 4 -> { board[row][col] = CellState.Dead }
      alive_neighbors == 3 -> { board[row][col] = CellState.Alive }
      _ -> {}
    }
  }
}
```

Enum members can also contain values inside them. You can declare what the values are using the type. For instance:

```nc
struct Element {
  str tag_name
  [str]str attributes
  Node[] children
}

enum Node {
  Root(Node[])
  Doctype
  Element(Element)
  Comment(str)
  Text(str)
}
```

You can get these values out by pattern matching on the type:

```nc
fn render(Node node) str {
  return if node {
    Node.Root(nodes) -> {
      mut str out = ""
      for node in nodes {
        out = out <> render(node)
      }
      out
    }
    Node.Doctype -> { "<!doctype html>" }
    Node.Element(el) -> { render_el(el) } // fn render_el(Element el) str
    Node.Comment(comment) -> { "<!--" <> comment <> "-->" }
    Node.Text(text) -> { text }
  }
}
```
