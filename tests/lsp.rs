//! Drives `ncc lsp` over stdio and checks diagnostics, hover, definition, symbols and completion.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Client {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to start ncc lsp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn send(&mut self, v: Value) {
        let body = v.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut len = 0;
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length: ") {
                len = v.parse().unwrap();
            }
        }
        let mut buf = vec![0; len];
        self.stdout.read_exact(&mut buf).unwrap();
        serde_json::from_slice(&buf).unwrap()
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let m = self.recv();
            if m.get("id") == Some(&json!(id)) {
                return m["result"].clone();
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Wait for diagnostics published for `uri`.
    fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        loop {
            let m = self.recv();
            if m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri {
                return m["params"]["diagnostics"].as_array().unwrap().clone();
            }
        }
    }

    /// Wait for diagnostics published for a version of an open document.
    fn diagnostics_at(&mut self, uri: &str, version: i64) -> Vec<Value> {
        loop {
            let m = self.recv();
            if m["method"] == "textDocument/publishDiagnostics"
                && m["params"]["uri"] == uri
                && m["params"]["version"] == version
            {
                return m["params"]["diagnostics"].as_array().unwrap().clone();
            }
        }
    }
}

fn pos(line: u32, character: u32) -> Value {
    json!({"line": line, "character": character})
}

#[test]
fn language_server() {
    let dir = std::env::temp_dir().join(format!("ncc-lsp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    let file = dir.join("main.nc");
    std::fs::write(&file, "").unwrap();
    let uri = format!("file://{}", file.display());

    let src = "struct Point {\n  int x\n  int y\n}\n\nfn add(int a, int b) int {\n  return a + b\n}\n\nPoint p = Point{.x = 1, .y = 2}\nint total = add(p.x, p.y)\n@println(total)\n";

    let mut c = Client::start();
    let init = c.request(
        "initialize",
        json!({"processId": null, "rootUri": null, "capabilities": {}}),
    );
    assert!(init["capabilities"]["hoverProvider"].as_bool().unwrap());
    c.notify("initialized", json!({}));

    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": "nc", "version": 1, "text": src}}),
    );
    assert!(c.diagnostics(&uri).is_empty(), "expected no diagnostics");

    // hover on `total` in `@println(total)`
    let h = c.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": pos(11, 10)}),
    );
    assert!(
        h["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("int total"),
        "{h}"
    );

    // hover on the call `add`
    let h = c.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": pos(10, 13)}),
    );
    assert!(
        h["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("fn add(int, int) int"),
        "{h}"
    );

    // go to definition of `add`
    let d = c.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": uri}, "position": pos(10, 13)}),
    );
    assert_eq!(d["range"]["start"]["line"], 5, "{d}");

    // go to definition of field `x` in `p.x`
    let d = c.request(
        "textDocument/definition",
        json!({"textDocument": {"uri": uri}, "position": pos(10, 18)}),
    );
    assert_eq!(d["range"]["start"]["line"], 1, "{d}");

    // document symbols
    let s = c.request(
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri}}),
    );
    let names: Vec<&str> = s
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Point", "add", "p", "total"]);

    let doc = json!({"uri": uri});

    // references to `add`: its declaration and the call
    let refs = c.request(
        "textDocument/references",
        json!({"textDocument": doc, "position": pos(10, 13), "context": {"includeDeclaration": true}}),
    );
    let lines: Vec<u64> = refs
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert_eq!(lines, [5, 10], "{refs}");

    // highlights of `p`: declaration and two uses
    let hl = c.request(
        "textDocument/documentHighlight",
        json!({"textDocument": doc, "position": pos(9, 6)}),
    );
    assert_eq!(hl.as_array().unwrap().len(), 3, "{hl}");

    // rename the field `x`: declaration, struct literal and field access
    let prep = c.request(
        "textDocument/prepareRename",
        json!({"textDocument": doc, "position": pos(1, 6)}),
    );
    assert_eq!(prep["placeholder"], "x", "{prep}");
    let edit = c.request(
        "textDocument/rename",
        json!({"textDocument": doc, "position": pos(1, 6), "newName": "horizontal"}),
    );
    let edits = edit["changes"][&uri].as_array().unwrap();
    let mut at: Vec<(u64, u64)> = edits
        .iter()
        .map(|e| {
            (
                e["range"]["start"]["line"].as_u64().unwrap(),
                e["range"]["start"]["character"].as_u64().unwrap(),
            )
        })
        .collect();
    at.sort_unstable();
    assert_eq!(at, [(1, 6), (9, 17), (10, 18)], "{edit}");
    assert!(edits.iter().all(|e| e["newText"] == "horizontal"));

    // signature help inside `add(p.x, |`
    let sig = c.request(
        "textDocument/signatureHelp",
        json!({"textDocument": doc, "position": pos(10, 21)}),
    );
    assert_eq!(
        sig["signatures"][0]["label"], "fn add(int a, int b) int",
        "{sig}"
    );
    assert_eq!(sig["activeParameter"], 1);

    // folding: the struct and the function body
    let folds = c.request("textDocument/foldingRange", json!({"textDocument": doc}));
    let starts: Vec<u64> = folds
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["startLine"].as_u64().unwrap())
        .collect();
    assert!(starts.contains(&0) && starts.contains(&5), "{folds}");

    // workspace symbols
    let ws = c.request("workspace/symbol", json!({"query": "ad"}));
    assert_eq!(ws[0]["name"], "add", "{ws}");

    // builtin hover
    let h = c.request(
        "textDocument/hover",
        json!({"textDocument": doc, "position": pos(11, 3)}),
    );
    assert!(
        h["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("fn @println"),
        "{h}"
    );

    // quick fix for an unused value
    let actions = c.request(
        "textDocument/codeAction",
        json!({"textDocument": doc, "range": {"start": pos(11, 0), "end": pos(11, 1)}, "context": {"diagnostics": [
            {"range": {"start": pos(11, 0), "end": pos(11, 5)}, "message": "return value of function not used (type `int`); assign it to `_` to discard it"}
        ]}}),
    );
    assert_eq!(
        actions[0]["edit"]["changes"][&uri][0]["newText"], "_ = ",
        "{actions}"
    );

    // formatting the (already formatted) document changes nothing
    let fmt = c.request(
        "textDocument/formatting",
        json!({"textDocument": doc, "options": {"tabSize": 2, "insertSpaces": true}}),
    );
    assert_eq!(fmt, json!([]), "{fmt}");

    // member completion after `p.`
    let edited = format!("{src}int z = p.\n");
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": edited}]}),
    );
    let diags = c.diagnostics(&uri);
    assert_eq!(diags.len(), 1, "expected a parse error: {diags:?}");
    let comp = c.request(
        "textDocument/completion",
        json!({"textDocument": {"uri": uri}, "position": pos(12, 10)}),
    );
    let labels: Vec<&str> = comp
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["x", "y"], "{comp}");

    // general completion includes keywords and declarations
    let comp = c.request(
        "textDocument/completion",
        json!({"textDocument": {"uri": uri}, "position": pos(12, 0)}),
    );
    let labels: Vec<&str> = comp
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["label"].as_str().unwrap())
        .collect();
    assert!(
        labels.contains(&"while") && labels.contains(&"add") && labels.contains(&"total"),
        "{labels:?}"
    );

    // type error diagnostic with position
    let bad = "int a = \"oops\"\n";
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 3}, "contentChanges": [{"text": bad}]}),
    );
    let diags = c.diagnostics(&uri);
    assert_eq!(diags.len(), 1);
    assert!(
        diags[0]["message"]
            .as_str()
            .unwrap()
            .contains("expected `int`, found `str`")
    );
    assert_eq!(diags[0]["range"]["start"], pos(0, 8));

    // warnings for data races
    let race = "mut int n = 0\nfn bump() int {\n  n = n + 1\n  return n\n}\nfut int f = async bump()\n_ = await f\n";
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 4}, "contentChanges": [{"text": race}]}),
    );
    let diags = c.diagnostics(&uri);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["severity"], 2);

    // a loop that never ends is information; the code after it is a faded warning
    let spin = "mut int n = 0\nwhile 1 < 2 {\n  n = n + 1\n}\n@println(n)\n@println(\"done\")\n";
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 41}, "contentChanges": [{"text": spin}]}),
    );
    let diags = c.diagnostics(&uri);
    assert_eq!(diags.len(), 2, "{diags:?}");
    let info = diags.iter().find(|d| d["code"] == "infinite-loop").unwrap();
    assert_eq!(info["severity"], 3);
    assert_eq!(info["range"]["start"], pos(1, 0));
    assert_eq!(info["range"]["end"], pos(1, 11));
    let dead = diags
        .iter()
        .find(|d| d["code"] == "unreachable-code")
        .unwrap();
    assert_eq!(dead["severity"], 2);
    assert_eq!(dead["tags"], json!([1]));
    assert_eq!(dead["range"]["start"], pos(4, 0));
    assert_eq!(dead["range"]["end"], pos(5, 16));

    // a loop left by `return` is not endless, but the code after it still never runs
    let ret = "fn f() int {\n  while true {\n    if {\n      @args().len > 1 -> { return 1 }\n      _ -> {}\n    }\n  }\n  return 0\n}\n@println(f())\n";
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 42}, "contentChanges": [{"text": ret}]}),
    );
    let diags = c.diagnostics(&uri);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "unreachable-code");

    // every error is published, not just the first
    let many = "int a = \"x\"\nstr b = 1\nint c = 1 / 0\nint d = (\n";
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 5}, "contentChanges": [{"text": many}]}),
    );
    let diags = c.diagnostics(&uri);
    assert_eq!(diags.len(), 4, "{diags:?}");

    c.request("shutdown", Value::Null);
    c.notify("exit", Value::Null);
    let status = c.child.wait().unwrap();
    assert!(status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Position of the `nth` occurrence (0-based) of `needle` in `src`, plus `delta` characters.
fn at(src: &str, needle: &str, nth: usize, delta: usize) -> Value {
    let mut from = 0;
    let mut off = 0;
    for _ in 0..=nth {
        off = from
            + src[from..]
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} not found"));
        from = off + 1;
    }
    let off = off + delta;
    let line = src[..off].matches('\n').count();
    let col = off - src[..off].rfind('\n').map_or(0, |i| i + 1);
    pos(line as u32, col as u32)
}

/// Completion labels, without snippets.
fn labels(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .filter(|x| x["kind"] != 15)
        .map(|x| x["label"].as_str().unwrap().to_string())
        .collect()
}

fn titles(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x["title"].as_str().unwrap().to_string())
        .collect()
}

const PROGRAM: &str = r#"import { "./util" as util }

/// A point on the plane
struct Point {
  /// horizontal position
  int x
  int y
}

enum Shape {
  Circle(int)
  Square
  Line
}

/// Adds two numbers
/// @param a the first
/// @param b the second
/// @returns the sum
fn add(int a, int b) int {
  return a + b
}

fn area(Shape s) int {
  if s {
    Shape.Circle(r) -> { return r * r * 3 }
    _ -> { return 0 }
  }
}

fn twice(int n) int {
  return add(n, n)
}

Point[] ps = [Point{.x = 1, .y = 2}]
int total = add(ps[0].x, 3)
for i in ps {
  @println(i)
}
@println(total, twice(2), area(Shape.Square), util.helper())
"#;

struct Session {
    c: Client,
    uri: String,
    dir: std::path::PathBuf,
    version: i64,
}

impl Session {
    fn start(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ncc-lsp-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        std::fs::write(
            dir.join("util.nc"),
            "/// Always one\npub fn helper() int {\n  return 1\n}\n",
        )
        .unwrap();
        let file = dir.join("main.nc");
        std::fs::write(&file, PROGRAM).unwrap();
        let uri = format!("file://{}", file.display());
        let mut c = Client::start();
        let root = format!("file://{}", dir.display());
        c.request(
            "initialize",
            json!({"processId": null, "rootUri": null, "workspaceFolders": [{"uri": root, "name": "ws"}], "capabilities": {
                "textDocument": {"completion": {"completionItem": {"snippetSupport": true}}}
            }}),
        );
        c.notify("initialized", json!({}));
        Self {
            c,
            uri,
            dir,
            version: 1,
        }
    }

    fn open(&mut self, text: &str) -> Vec<Value> {
        self.c.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": self.uri, "languageId": "nc", "version": 1, "text": text}}),
        );
        self.diags()
    }

    /// Diagnostics published for the current version of the document.
    fn diags(&mut self) -> Vec<Value> {
        loop {
            let m = self.c.recv();
            if m["method"] == "textDocument/publishDiagnostics"
                && m["params"]["uri"] == self.uri.as_str()
                && m["params"]["version"] == self.version
            {
                return m["params"]["diagnostics"].as_array().unwrap().clone();
            }
        }
    }

    fn change(&mut self, text: &str) -> Vec<Value> {
        self.version += 1;
        self.c.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": self.uri, "version": self.version}, "contentChanges": [{"text": text}]}),
        );
        self.diags()
    }

    fn req(&mut self, method: &str, mut params: Value) -> Value {
        params["textDocument"] = json!({"uri": self.uri});
        self.c.request(method, params)
    }

    fn actions(&mut self, range: (Value, Value), diags: &[Value]) -> Value {
        self.req(
            "textDocument/codeAction",
            json!({"range": {"start": range.0, "end": range.1}, "context": {"diagnostics": diags}}),
        )
    }

    fn finish(mut self) {
        self.c.request("shutdown", Value::Null);
        self.c.notify("exit", Value::Null);
        assert!(self.c.child.wait().unwrap().success());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn hover_hints_and_navigation() {
    let mut s = Session::start("nav");
    let src = PROGRAM;
    assert_eq!(s.open(src), Vec::<Value>::new());

    // doc comments in hover, with parameters and the return value
    let h = s.req(
        "textDocument/hover",
        json!({"position": at(src, "add(ps", 0, 1)}),
    );
    let v = h["contents"]["value"].as_str().unwrap();
    assert!(v.contains("Adds two numbers"), "{v}");
    assert!(v.contains("- `a`: the first"), "{v}");
    assert!(v.contains("**Returns** the sum"), "{v}");
    // ... on fields, and across modules
    let h = s.req(
        "textDocument/hover",
        json!({"position": at(src, "].x", 0, 2)}),
    );
    assert!(
        h["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("horizontal position"),
        "{h}"
    );
    let h = s.req(
        "textDocument/hover",
        json!({"position": at(src, "helper", 0, 1)}),
    );
    let v = h["contents"]["value"].as_str().unwrap();
    assert!(
        v.contains("Always one") && v.contains("Defined in `util.nc`"),
        "{v}"
    );
    // the type of an expression without a symbol
    let h = s.req(
        "textDocument/hover",
        json!({"position": at(src, "[Point{", 0, 0)}),
    );
    assert_eq!(h["contents"]["value"], "```nc\nPoint[]\n```", "{h}");

    // keywords are documented
    let h = s.req(
        "textDocument/hover",
        json!({"position": at(src, "for i", 0, 1)}),
    );
    assert!(
        h["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("loops over the indices"),
        "{h}"
    );

    // inlay hints: parameter names at calls, types of bound variables
    let hints = s.req(
        "textDocument/inlayHint",
        json!({"range": {"start": pos(0, 0), "end": pos(100, 0)}}),
    );
    let hs: Vec<(Value, String)> = hints
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["position"].clone(),
                h["label"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(
        hs.contains(&(at(src, "ps[0].x, 3", 0, 0), "a:".into())),
        "{hs:?}"
    );
    assert!(hs.contains(&(at(src, "3)", 0, 0), "b:".into())), "{hs:?}");
    assert!(
        hs.contains(&(at(src, "r) ->", 0, 0), "int".into())),
        "{hs:?}"
    );
    assert!(
        hs.contains(&(at(src, "Shape.Square)", 0, 0), "s:".into())),
        "{hs:?}"
    );
    assert!(
        hs.iter()
            .any(|(p, l)| *p == at(src, "i in", 0, 0) && l.contains("int")),
        "{hs:?}"
    );

    // go to type definition: from a variable to its struct
    let d = s.req(
        "textDocument/typeDefinition",
        json!({"position": at(src, "ps[0]", 0, 0)}),
    );
    assert_eq!(d["range"]["start"], at(src, "Point {", 0, 0), "{d}");
    // declaration is the definition
    let d = s.req(
        "textDocument/declaration",
        json!({"position": at(src, "twice(2)", 0, 0)}),
    );
    assert_eq!(
        d["range"]["start"]["line"],
        at(src, "fn twice", 0, 0)["line"],
        "{d}"
    );

    // semantic tokens: `add` is a function declaration, `a` a readonly parameter
    let t = s.req("textDocument/semanticTokens/full", json!({}));
    let data: Vec<u64> = t["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap())
        .collect();
    let (mut line, mut col) = (0, 0);
    let mut toks = Vec::new();
    for c in data.chunks(5) {
        if c[0] > 0 {
            line += c[0];
            col = c[1];
        } else {
            col += c[1];
        }
        toks.push((line, col, c[2], c[3], c[4]));
    }
    let find = |p: Value| {
        toks.iter()
            .find(|t| t.0 == p["line"].as_u64().unwrap() && t.1 == p["character"].as_u64().unwrap())
            .copied()
            .unwrap_or_else(|| panic!("no token at {p}"))
    };
    let add_decl = find(at(src, "add(int", 0, 0));
    assert_eq!(
        (add_decl.3, add_decl.4 & 1),
        (5, 1),
        "function declaration {add_decl:?}"
    );
    let param_use = find(at(src, "a + b", 0, 0));
    assert_eq!(
        (param_use.3, param_use.4 & 2),
        (6, 2),
        "readonly parameter {param_use:?}"
    );
    let ns = find(at(src, "util.helper", 0, 0));
    assert_eq!(ns.3, 0, "namespace {ns:?}");

    // call hierarchy of `add`: called from `twice` and the top level; `twice` calls `add`
    let items = s.req(
        "textDocument/prepareCallHierarchy",
        json!({"position": at(src, "add(int", 0, 0)}),
    );
    assert_eq!(items[0]["name"], "add", "{items}");
    let inc =
        s.c.request("callHierarchy/incomingCalls", json!({"item": items[0]}));
    let mut from: Vec<String> = inc
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["from"]["name"].as_str().unwrap().to_string())
        .collect();
    from.sort();
    assert_eq!(from, ["main.nc", "twice"], "{inc}");
    let tw = s.req(
        "textDocument/prepareCallHierarchy",
        json!({"position": at(src, "twice(int", 0, 0)}),
    );
    let out =
        s.c.request("callHierarchy/outgoingCalls", json!({"item": tw[0]}));
    assert_eq!(out[0]["to"]["name"], "add", "{out}");

    // selection ranges grow from the name outwards
    let sel = s.req(
        "textDocument/selectionRange",
        json!({"positions": [at(src, "].x", 0, 2)]}),
    );
    let mut depth = 0;
    let mut r = &sel[0];
    let first = r["range"].clone();
    while !r.is_null() {
        depth += 1;
        r = &r["parent"];
    }
    assert_eq!(first["start"], at(src, "].x", 0, 2), "{sel}");
    assert!(depth >= 4, "{sel}");

    // document links to imported modules
    let links = s.req("textDocument/documentLink", json!({}));
    assert!(
        links[0]["target"].as_str().unwrap().ends_with("/util.nc"),
        "{links}"
    );
    assert_eq!(links[0]["range"]["start"], pos(0, 10), "{links}");

    // signature help with documentation
    let sig = s.req(
        "textDocument/signatureHelp",
        json!({"position": at(src, "ps[0].x, 3", 0, 0)}),
    );
    assert!(
        sig["signatures"][0]["documentation"]["value"]
            .as_str()
            .unwrap()
            .contains("Adds two"),
        "{sig}"
    );
    assert!(
        sig["signatures"][0]["parameters"][0]["documentation"]["value"]
            .as_str()
            .unwrap()
            .contains("the first"),
        "{sig}"
    );

    // commas in strings do not count as argument separators
    let text = format!("{src}int q = add(\"a,b\".len, 1)\n");
    s.change(&text);
    let sig = s.req(
        "textDocument/signatureHelp",
        json!({"position": at(&text, ".len, 1", 0, 3)}),
    );
    assert_eq!(sig["activeParameter"], 0, "{sig}");
    s.change(src);

    // renaming to a name that is already declared is refused
    let r = s.c.request(
        "textDocument/rename",
        json!({"textDocument": {"uri": s.uri}, "position": at(src, "twice(int", 0, 0), "newName": "add"}),
    );
    assert!(r.is_null(), "{r}");
    let r = s.c.request(
        "textDocument/rename",
        json!({"textDocument": {"uri": s.uri}, "position": at(src, "twice(int", 0, 0), "newName": "double"}),
    );
    assert_eq!(r["changes"][&s.uri].as_array().unwrap().len(), 2, "{r}");

    // reference counts above declarations
    let lenses = s.req("textDocument/codeLens", json!({}));
    let add = lenses
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["range"]["start"] == at(src, "add(int", 0, 0))
        .unwrap_or_else(|| panic!("{lenses}"));
    assert_eq!(add["command"]["title"], "2 references", "{add}");

    // references and rename include files that are not open
    let refs = s.req(
        "textDocument/references",
        json!({"position": at(src, "helper", 0, 0), "context": {"includeDeclaration": true}}),
    );
    assert_eq!(refs.as_array().unwrap().len(), 2, "{refs}");

    s.finish();
}

#[test]
fn completion_features() {
    let mut s = Session::start("complete");
    let src = PROGRAM;
    s.open(src);

    // members of an indexed expression, with documentation
    let text = format!("{src}int w = ps[0].\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "ps[0].\n", 0, 6)}),
    );
    assert_eq!(labels(&comp), ["x", "y"], "{comp}");
    assert!(
        comp[0]["documentation"]["value"]
            .as_str()
            .unwrap()
            .contains("horizontal")
    );

    // members of a call's result
    let text = format!("{src}int len = ps.\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "ps.\n", 0, 3)}),
    );
    assert_eq!(labels(&comp), ["len"], "{comp}");

    // struct literal fields that are not set yet
    let text = format!("{src}Point q = Point{{.x = 1, .\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, ", .\n", 0, 3)}),
    );
    assert_eq!(labels(&comp), ["y"], "{comp}");
    assert_eq!(comp[0]["insertText"], "y = ");

    // enum variants, with a snippet for the payload
    let text = format!("{src}Shape sh = Shape.\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "Shape.\n", 0, 6)}),
    );
    assert_eq!(labels(&comp), ["Circle", "Square", "Line"], "{comp}");
    assert_eq!(comp[0]["insertText"], "Circle($1)$0");

    // module members
    let text = format!("{src}int h = util.\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "util.\n", 0, 5)}),
    );
    assert_eq!(labels(&comp), ["helper"], "{comp}");
    assert!(
        comp[0]["documentation"]["value"]
            .as_str()
            .unwrap()
            .contains("Always one")
    );

    // scope: a function's parameters are not visible outside it; functions get call snippets
    let text = format!("{src}\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "helper())\n", 0, 10)}),
    );
    let ls = labels(&comp);
    assert!(
        ls.contains(&"total".into()) && !ls.contains(&"n".into()) && !ls.contains(&"r".into()),
        "{ls:?}"
    );
    let add = comp
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "add")
        .unwrap();
    assert_eq!(add["insertText"], "add(${1:a}, ${2:b})$0", "{add}");
    // inside `twice`, its parameter is visible
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(src, "return add(n", 0, 7)}),
    );
    assert!(labels(&comp).contains(&"n".into()));

    // import paths
    let text = format!("import {{ \"./\n{src}");
    s.change(&text);
    let comp = s.req("textDocument/completion", json!({"position": pos(0, 12)}));
    assert_eq!(labels(&comp), ["util"], "{comp}");

    // modules that are not imported yet, adding the import
    std::fs::write(
        s.dir.join("geometry.nc"),
        "pub fn pi() float {\n  return 3.14\n}\n",
    )
    .unwrap();
    let text = format!("{src}\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "helper())\n", 0, 10)}),
    );
    let g = comp
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "geometry")
        .unwrap_or_else(|| panic!("{comp}"));
    assert_eq!(
        g["additionalTextEdits"][0]["newText"],
        "import { \"./geometry\" as geometry }\n"
    );
    assert_eq!(g["additionalTextEdits"][0]["range"]["start"], pos(1, 0));
    let text = format!("{src}float f = geometry.\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "geometry.\n", 0, 9)}),
    );
    assert_eq!(labels(&comp), ["pi"], "{comp}");
    assert!(comp[0]["additionalTextEdits"].is_array());

    // inside string interpolations: members and names
    let text = format!("{src}@println(\"w = {{ps[0].}}\")\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "ps[0].}", 0, 6)}),
    );
    assert_eq!(labels(&comp), ["x", "y"], "{comp}");
    let text = format!("{src}@println(\"t = {{tot}}\")\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "tot}", 0, 3)}),
    );
    assert!(labels(&comp).contains(&"total".into()), "{comp}");
    // but not in the string's own text
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "t = ", 0, 2)}),
    );
    assert_eq!(labels(&comp), Vec::<String>::new(), "{comp}");

    // builtins with documentation
    let text = format!("{src}@\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "@\n", 0, 1)}),
    );
    assert!(labels(&comp).contains(&"embed".into()));

    s.finish();
}

#[test]
fn diagnostics_and_code_actions() {
    let mut s = Session::start("actions");
    let src = PROGRAM;
    s.open(src);

    // unused variables and imports are faded warnings with a fix
    let text = src.replace(", util.helper()", "") + "fn f(int unused) {\n  int also = 1\n}\nf(1)\n";
    let diags = s.change(&text);
    let mut msgs: Vec<&str> = diags
        .iter()
        .map(|d| d["message"].as_str().unwrap())
        .collect();
    msgs.sort_unstable();
    assert_eq!(
        msgs,
        [
            "unused import `util`",
            "unused parameter `unused`; prefix the name with `_` if this is intended",
            "unused variable `also`; prefix the name with `_` if this is intended"
        ],
        "{diags:?}"
    );
    assert!(
        diags
            .iter()
            .all(|d| d["tags"] == json!([1]) && d["severity"] == 2)
    );
    let d = diags
        .iter()
        .find(|d| d["code"] == "unused-variable")
        .unwrap()
        .clone();
    let acts = s.actions(
        (d["range"]["start"].clone(), d["range"]["end"].clone()),
        std::slice::from_ref(&d),
    );
    assert_eq!(
        titles(&acts)[..2],
        ["Prefix the name with `_`", "Rename to `_`"],
        "{acts}"
    );
    let e = &acts[0]["edit"]["changes"][&s.uri][0];
    assert_eq!(
        (e["newText"].clone(), e["range"]["start"].clone()),
        (json!("_"), d["range"]["start"].clone())
    );
    assert_eq!(acts[1]["edit"]["changes"][&s.uri][0]["range"], d["range"]);
    let d = diags
        .iter()
        .find(|d| d["code"] == "unused-import")
        .unwrap()
        .clone();
    let acts = s.actions(
        (d["range"]["start"].clone(), d["range"]["end"].clone()),
        std::slice::from_ref(&d),
    );
    assert!(
        titles(&acts).contains(&"Remove unused import".to_string()),
        "{acts}"
    );
    let e = &acts[0]["edit"]["changes"][&s.uri][0];
    assert_eq!(
        (
            e["range"]["start"].clone(),
            e["range"]["end"].clone(),
            e["newText"].clone()
        ),
        (pos(0, 0), pos(1, 0), json!(""))
    );

    // missing `if` branches
    let text = format!(
        "{src}fn g(Shape s) int {{\n  if s {{\n    Shape.Square -> {{ return 1 }}\n  }}\n}}\n"
    );
    let diags = s.change(&text);
    let d = diags
        .iter()
        .find(|d| d["message"].as_str().unwrap().contains("not exhaustive"))
        .unwrap()
        .clone();
    let acts = s.actions(
        (d["range"]["start"].clone(), d["range"]["start"].clone()),
        std::slice::from_ref(&d),
    );
    assert_eq!(
        titles(&acts)[..2],
        ["Add missing branches", "Add a fallback `_` branch"],
        "{acts}"
    );
    let e = &acts[0]["edit"]["changes"][&s.uri][0];
    assert_eq!(
        e["newText"], "    Shape.Circle(_) -> {}\n    Shape.Line -> {}\n",
        "{e}"
    );
    assert_eq!(e["range"]["start"], at(&text, "  }\n}\n", 1, 0), "{e}");

    // missing struct fields
    let text = format!("{src}Point m = Point{{.x = 1}}\n@println(m)\n");
    let diags = s.change(&text);
    let d = diags[0].clone();
    let acts = s.actions(
        (d["range"]["start"].clone(), d["range"]["start"].clone()),
        std::slice::from_ref(&d),
    );
    assert_eq!(acts[0]["title"], "Add missing fields", "{acts}");
    assert_eq!(acts[0]["edit"]["changes"][&s.uri][0]["newText"], ", .y = 0");

    // misspelt names
    let text = format!("{src}int t2 = totl\n@println(t2)\n");
    let diags = s.change(&text);
    let d = diags[0].clone();
    let acts = s.actions(
        (d["range"]["start"].clone(), d["range"]["start"].clone()),
        std::slice::from_ref(&d),
    );
    assert_eq!(acts[0]["title"], "Change to `total`", "{acts}");

    // throwing outside a throwing function
    let text = format!("{src}fn h() int {{\n  throw \"no\"\n}}\n");
    let diags = s.change(&text);
    let d = diags[0].clone();
    let acts = s.actions(
        (d["range"]["start"].clone(), d["range"]["start"].clone()),
        std::slice::from_ref(&d),
    );
    assert_eq!(acts[0]["title"], "Make `h` throwing", "{acts}");
    let e = &acts[0]["edit"]["changes"][&s.uri][0];
    assert_eq!(
        (e["range"]["start"].clone(), e["newText"].clone()),
        (at(&text, "int {\n  throw", 0, 3), json!("!"))
    );

    // extract an expression into a variable
    s.change(src);
    let (a, b) = (at(src, "add(ps", 0, 0), at(src, "\nfor i", 0, 0));
    let acts = s.actions((a, b), &[]);
    let ex = acts
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["title"] == "Extract into a variable")
        .unwrap_or_else(|| panic!("{acts}"));
    let edits = ex["edit"]["changes"][&s.uri].as_array().unwrap();
    assert_eq!(edits[0]["newText"], "int value = add(ps[0].x, 3)\n");
    assert_eq!(edits[1]["newText"], "value");

    // generate a function that does not exist yet
    let text = format!("{src}int got = compute(total, \"x\")\n@println(got)\n");
    let diags = s.change(&text);
    let d = diags[0].clone();
    let acts = s.actions(
        (d["range"]["start"].clone(), d["range"]["start"].clone()),
        std::slice::from_ref(&d),
    );
    let g = acts
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["title"] == "Generate function `compute`")
        .unwrap_or_else(|| panic!("{acts}"));
    assert_eq!(
        g["edit"]["changes"][&s.uri][0]["newText"],
        "\nfn compute(int total, str arg2) int {\n}\n"
    );

    // match on an enum value
    let text = format!("{src}Shape sh = Shape.Line\n");
    s.change(&text);
    let acts = s.actions((at(&text, "sh =", 0, 0), at(&text, "sh =", 0, 0)), &[]);
    let m = acts
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["title"] == "Match on `sh`")
        .unwrap_or_else(|| panic!("{acts}"));
    assert_eq!(
        m["edit"]["changes"][&s.uri][0]["newText"],
        "if sh {\n  Shape.Circle(_) -> {}\n  Shape.Square -> {}\n  Shape.Line -> {}\n}\n"
    );
    s.change(src);

    // inline a variable
    let acts = s.actions((at(src, "total =", 0, 0), at(src, "total =", 0, 0)), &[]);
    let inl = acts
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["title"] == "Inline `total`")
        .unwrap_or_else(|| panic!("{acts}"));
    let edits = inl["edit"]["changes"][&s.uri].as_array().unwrap();
    assert_eq!(edits[0]["newText"], "add(ps[0].x, 3)");
    assert_eq!(edits[1]["newText"], "");

    // incremental changes are applied to the document
    s.version += 1;
    let v = s.version;
    s.c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": s.uri, "version": v}, "contentChanges": [
            {"range": {"start": at(src, "int total", 0, 4), "end": at(src, "int total", 0, 9)}, "text": "sum"},
            {"range": {"start": at(src, "@println(total", 0, 9), "end": at(src, "@println(total", 0, 14)}, "text": "sum"}
        ]}),
    );
    assert_eq!(s.diags(), Vec::<Value>::new());
    let h = s.req(
        "textDocument/hover",
        json!({"position": at(src, "@println(total", 0, 10)}),
    );
    assert!(
        h["contents"]["value"].as_str().unwrap().contains("int sum"),
        "{h}"
    );

    // formatting only touches the lines that change
    let messy = src.replace("int total = add", "int   total =  add");
    s.change(&messy);
    let edits = s.req(
        "textDocument/formatting",
        json!({"options": {"tabSize": 2, "insertSpaces": true}}),
    );
    let line = at(src, "int total", 0, 0)["line"].clone();
    assert_eq!(edits.as_array().unwrap().len(), 1, "{edits}");
    assert_eq!(edits[0]["range"]["start"]["line"], line);
    assert_eq!(edits[0]["newText"], "int total = add(ps[0].x, 3)\n");

    s.finish();
}

#[test]
fn workspace_features() {
    let dir = std::env::temp_dir().join(format!("ncc-lsp-ws-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".nc")).unwrap();
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    std::fs::write(dir.join(".nc/config.toml"), "").unwrap();
    std::fs::write(
        dir.join("lib/shapes.nc"),
        "pub fn sides() int {\n  return 4\n}\n",
    )
    .unwrap();
    // a file that is never opened, with an error
    std::fs::write(dir.join("broken.nc"), "int a = \"no\"\n").unwrap();
    let main =
        "import { \"./lib/shapes\" as shapes }\n\n@println(shapes.sides(), \"{shapes.sides()}\")\n";
    std::fs::write(dir.join("main.nc"), main).unwrap();
    let uri = format!("file://{}", dir.join("main.nc").display());
    let broken = format!("file://{}", dir.join("broken.nc").display());

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": null,
            "workspaceFolders": [{"uri": format!("file://{}", dir.display()), "name": "ws"}],
            "capabilities": {}
        }),
    );
    c.notify("initialized", json!({}));
    // diagnostics for the whole workspace, not only open files
    let d = c.diagnostics(&broken);
    assert_eq!(d.len(), 1, "{d:?}");

    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": "nc", "version": 1, "text": main}}),
    );
    assert_eq!(c.diagnostics_at(&uri, 1), Vec::<Value>::new());
    let doc = json!({"uri": uri});

    // an import alias: references (also in string interpolations), rename, hover, definition
    let refs = c.request(
        "textDocument/references",
        json!({"textDocument": doc, "position": at(main, "shapes.sides", 0, 2), "context": {"includeDeclaration": true}}),
    );
    assert_eq!(refs.as_array().unwrap().len(), 3, "{refs}");
    let edit = c.request(
        "textDocument/rename",
        json!({"textDocument": doc, "position": at(main, "as shapes", 0, 4), "newName": "sh"}),
    );
    assert_eq!(edit["changes"][&uri].as_array().unwrap().len(), 3, "{edit}");
    let h = c.request(
        "textDocument/hover",
        json!({"textDocument": doc, "position": at(main, "shapes.sides", 0, 1)}),
    );
    assert!(
        h["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("module ./lib/shapes"),
        "{h}"
    );
    let d = c.request(
        "textDocument/definition",
        json!({"textDocument": doc, "position": at(main, "shapes.sides", 0, 1)}),
    );
    assert!(
        d["uri"].as_str().unwrap().ends_with("/lib/shapes.nc"),
        "{d}"
    );

    // renaming a module file updates the imports of it
    let old = format!("file://{}", dir.join("lib/shapes.nc").display());
    let new = format!("file://{}", dir.join("lib/polygons.nc").display());
    let edit = c.request(
        "workspace/willRenameFiles",
        json!({"files": [{"oldUri": old, "newUri": new}]}),
    );
    let e = &edit["changes"][&uri][0];
    assert_eq!(e["newText"], "./lib/polygons", "{edit}");
    assert_eq!(e["range"]["start"], pos(0, 10), "{edit}");

    // a module used without an import: offer to import it
    let text = "@println(shapes.sides())\n";
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": text}]}),
    );
    let diags = c.diagnostics_at(&uri, 2);
    let acts = c.request(
        "textDocument/codeAction",
        json!({"textDocument": doc, "range": {"start": pos(0, 9), "end": pos(0, 9)}, "context": {"diagnostics": diags}}),
    );
    assert_eq!(
        titles(&acts)[0],
        "Import `.../lib/shapes` as `shapes`",
        "{acts}"
    );
    let e = &acts[0]["edit"]["changes"][&uri][0];
    assert_eq!(
        e["newText"], "import { \".../lib/shapes\" as shapes }\n\n",
        "{e}"
    );

    // document symbols carry details and select the name
    std::fs::write(
        dir.join("lib/shapes.nc"),
        "pub fn sides() int {\n  return 4\n}\n",
    )
    .unwrap();
    let text = "struct P {\n  int x\n}\nfn area(int w, int h) int {\n  fn inner() int {\n    return 1\n  }\n  return w * h\n}\n";
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 3}, "contentChanges": [{"text": text}]}),
    );
    c.diagnostics_at(&uri, 3);
    let syms = c.request("textDocument/documentSymbol", json!({"textDocument": doc}));
    assert_eq!(syms[1]["detail"], "(int w, int h) int", "{syms}");
    assert_eq!(syms[1]["selectionRange"]["start"], pos(3, 3), "{syms}");
    assert_eq!(syms[1]["children"][0]["name"], "inner", "{syms}");
    assert_eq!(syms[0]["children"][0]["detail"], "int", "{syms}");

    c.request("shutdown", Value::Null);
    c.notify("exit", Value::Null);
    assert!(c.child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&dir);
}

fn find_action<'a>(acts: &'a Value, title: &str) -> &'a Value {
    acts.as_array()
        .unwrap()
        .iter()
        .find(|x| x["title"] == title)
        .unwrap_or_else(|| panic!("no `{title}` in {acts}"))
}

#[test]
fn more_code_actions() {
    let mut s = Session::start("more");
    let src = PROGRAM;
    s.open(src);
    let uri = s.uri.clone();
    let first_diag = |s: &mut Session, text: &str| -> Value {
        let d = s.change(text);
        assert!(!d.is_empty(), "expected a diagnostic for {text}");
        d[0].clone()
    };
    let fix = |s: &mut Session, d: &Value| -> Value {
        s.actions(
            (d["range"]["start"].clone(), d["range"]["start"].clone()),
            std::slice::from_ref(d),
        )
    };

    // generate a missing type
    let text = format!("{src}Foo f = 1\n");
    let d = first_diag(&mut s, &text);
    let acts = fix(&mut s, &d);
    let a = find_action(&acts, "Generate struct `Foo`");
    assert_eq!(
        a["edit"]["changes"][&uri][0]["newText"],
        "\nstruct Foo {\n}\n"
    );

    // create a missing module
    let text = src.replace("\"./util\" as util", "\"./nope\" as util");
    let d = first_diag(&mut s, &text);
    let acts = fix(&mut s, &d);
    let a = find_action(&acts, "Create module `./nope`");
    let op = &a["edit"]["documentChanges"][0];
    assert_eq!(op["kind"], "create");
    assert!(op["uri"].as_str().unwrap().ends_with("/nope.nc"), "{op}");

    // make an item of another module public
    std::fs::write(
        s.dir.join("util.nc"),
        "/// Always one\npub fn helper() int {\n  return 1\n}\n\nfn secret() int {\n  return 2\n}\n",
    )
    .unwrap();
    let text = format!("{src}@println(util.secret())\n");
    let d = first_diag(&mut s, &text);
    let acts = fix(&mut s, &d);
    let a = find_action(&acts, "Make `secret` public");
    let util_uri = format!("file://{}", s.dir.join("util.nc").display());
    let e = &a["edit"]["changes"][&util_uri][0];
    assert_eq!(
        (e["newText"].clone(), e["range"]["start"].clone()),
        (json!("pub "), pos(5, 0))
    );

    // fill the fields a struct pattern leaves out
    let text = format!("{src}if ps[0] {{\n  Point{{.x = 1}} -> {{}}\n  _ -> {{}}\n}}\n");
    let d = first_diag(&mut s, &text);
    let acts = fix(&mut s, &d);
    let a = find_action(&acts, "Fill in the missing fields with `_`");
    assert_eq!(a["edit"]["changes"][&uri][0]["newText"], ", .y = _");

    s.change(src);
    let at_cursor = |s: &mut Session, p: Value| s.actions((p.clone(), p), &[]);

    // comments
    let acts = at_cursor(&mut s, at(src, "/// A point", 0, 4));
    let a = find_action(&acts, "Convert to a regular comment");
    assert_eq!(a["edit"]["changes"][&uri][0]["newText"], "//");

    // integer bases
    let acts = at_cursor(&mut s, at(src, "3)\n", 0, 0));
    find_action(&acts, "Convert to hexadecimal (`0x3`)");
    find_action(&acts, "Convert to binary (`0b11`)");

    // extract a constant out of a function
    let (a0, a1) = (at(src, "3 }", 0, 0), at(src, "3 }", 0, 1));
    let acts = s.actions((a0, a1), &[]);
    let a = find_action(&acts, "Extract into a module-level constant");
    let edits = a["edit"]["changes"][&uri].as_array().unwrap();
    assert_eq!(edits[0]["newText"], "int CONSTANT = 3\n\n");
    assert_eq!(edits[0]["range"]["start"], at(src, "fn area", 0, 0));
    assert_eq!(edits[1]["newText"], "CONSTANT");

    // interpolate a selection into its string
    let text = format!("{src}str name = \"x\"\n@println(\"hello name\")\n");
    s.change(&text);
    let (a0, a1) = (at(&text, "name\")", 0, 0), at(&text, "name\")", 0, 4));
    let acts = s.actions((a0, a1), &[]);
    let a = find_action(&acts, "Interpolate `name`");
    assert_eq!(a["edit"]["changes"][&uri][0]["newText"], "{name}");

    // match on the result of a call
    let text =
        format!("{src}fn pick() Shape {{\n  return Shape.Line\n}}\ntest \"t\" {{\n  pick()\n}}\n");
    s.change(&text);
    let acts = at_cursor(&mut s, at(&text, "pick()\n}", 0, 1));
    let a = find_action(&acts, "Match on the result");
    assert_eq!(
        a["edit"]["changes"][&uri][0]["newText"],
        "if pick() {\n    Shape.Circle(_) -> {}\n    Shape.Square -> {}\n    Shape.Line -> {}\n  }"
    );

    // unreachable branches are faded warnings that can be removed
    let text = format!(
        "{src}fn kind(int n) str {{\n  if n {{\n    1 -> {{ return \"one\" }}\n    _ -> {{ return \"many\" }}\n    2 -> {{ return \"two\" }}\n  }}\n}}\nfn same(Shape sh) int {{\n  if sh {{\n    Shape.Square -> {{ return 1 }}\n    Shape.Square -> {{ return 2 }}\n    _ -> {{ return 3 }}\n  }}\n}}\n"
    );
    let diags = s.change(&text);
    let un: Vec<&Value> = diags
        .iter()
        .filter(|d| d["code"] == "unreachable-branch")
        .collect();
    assert_eq!(un.len(), 2, "{diags:?}");
    assert_eq!(un[0]["range"]["start"], at(&text, "2 -> {", 0, 0));
    assert_eq!(
        un[1]["range"]["start"],
        at(&text, "Shape.Square -> { return 2", 0, 0)
    );
    let acts = fix(&mut s, un[0]);
    let a = find_action(&acts, "Remove the unreachable branch");
    let e = &a["edit"]["changes"][&uri][0];
    assert_eq!(e["range"]["start"]["character"], 0);
    assert_eq!(
        e["range"]["end"]["line"],
        un[0]["range"]["start"]["line"].as_u64().unwrap() + 1
    );

    // postfix completions wrap the receiver
    let text = format!("{src}total.\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "total.\n", 0, 6)}),
    );
    let post = comp
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "println")
        .unwrap_or_else(|| panic!("{comp}"));
    assert_eq!(post["textEdit"]["newText"], "@println(total)");
    assert_eq!(
        post["textEdit"]["range"]["start"],
        at(&text, "total.\n", 0, 0)
    );

    // typing `}` re-indents it to its `{`
    let text = format!("{src}fn z() {{\n  @println(1)\n  }}\n");
    s.change(&text);
    let e = s.req("textDocument/onTypeFormatting", json!({"position": at(&text, "  }\n", 1, 3), "ch": "}", "options": {"tabSize": 2, "insertSpaces": true}}));
    assert_eq!(e[0]["newText"], "", "{e}");
    assert_eq!(e[0]["range"]["end"], at(&text, "  }\n", 1, 2), "{e}");

    // the syntax tree, for debugging
    let tree = s.req("ncc/syntaxTree", json!({}));
    assert!(tree.as_str().unwrap().contains("FnDecl"), "{tree}");

    // exit points of a function
    s.change(src);
    let hl = s.req(
        "textDocument/documentHighlight",
        json!({"position": at(src, "fn area", 0, 1)}),
    );
    let lines: Vec<u64> = hl
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    let l = |n: &str| at(src, n, 0, 0)["line"].as_u64().unwrap();
    assert_eq!(
        lines,
        [l("fn area"), l("return r * r"), l("return 0")],
        "{hl}"
    );

    s.finish();
}

/// Every response deserializes into the `lsp-types` structures a typed client would use.
#[test]
fn responses_follow_the_protocol() {
    use lsp_types::*;
    fn check<T: serde::de::DeserializeOwned>(method: &str, v: Value) {
        if let Err(e) = serde_json::from_value::<T>(v.clone()) {
            panic!("{method}: {e}\n{v}");
        }
    }
    let mut s = Session::start("typed");
    let src = PROGRAM;
    s.open(src);
    let p = at(src, "add(ps", 0, 1);
    let r = json!({"start": pos(0, 0), "end": pos(40, 0)});
    check::<Option<Hover>>("hover", s.req("textDocument/hover", json!({"position": p})));
    check::<Option<GotoDefinitionResponse>>(
        "definition",
        s.req("textDocument/definition", json!({"position": p})),
    );
    check::<Option<GotoDefinitionResponse>>(
        "typeDefinition",
        s.req(
            "textDocument/typeDefinition",
            json!({"position": at(src, "ps[0]", 0, 0)}),
        ),
    );
    check::<Option<CompletionResponse>>(
        "completion",
        s.req("textDocument/completion", json!({"position": p})),
    );
    check::<Option<SignatureHelp>>(
        "signatureHelp",
        s.req(
            "textDocument/signatureHelp",
            json!({"position": at(src, "3)", 0, 0)}),
        ),
    );
    check::<Option<Vec<Location>>>(
        "references",
        s.req(
            "textDocument/references",
            json!({"position": p, "context": {"includeDeclaration": true}}),
        ),
    );
    check::<Option<Vec<DocumentHighlight>>>(
        "documentHighlight",
        s.req("textDocument/documentHighlight", json!({"position": p})),
    );
    check::<Option<PrepareRenameResponse>>(
        "prepareRename",
        s.req("textDocument/prepareRename", json!({"position": p})),
    );
    check::<Option<WorkspaceEdit>>(
        "rename",
        s.req(
            "textDocument/rename",
            json!({"position": p, "newName": "plus"}),
        ),
    );
    check::<Option<DocumentSymbolResponse>>(
        "documentSymbol",
        s.req("textDocument/documentSymbol", json!({})),
    );
    check::<Option<WorkspaceSymbolResponse>>(
        "workspace/symbol",
        s.c.request("workspace/symbol", json!({"query": ""})),
    );
    check::<Option<Vec<FoldingRange>>>(
        "foldingRange",
        s.req("textDocument/foldingRange", json!({})),
    );
    check::<Option<Vec<SelectionRange>>>(
        "selectionRange",
        s.req("textDocument/selectionRange", json!({"positions": [p]})),
    );
    check::<Option<Vec<DocumentLink>>>(
        "documentLink",
        s.req("textDocument/documentLink", json!({})),
    );
    check::<Option<Vec<InlayHint>>>(
        "inlayHint",
        s.req("textDocument/inlayHint", json!({"range": r})),
    );
    check::<Option<SemanticTokensResult>>(
        "semanticTokens",
        s.req("textDocument/semanticTokens/full", json!({})),
    );
    check::<Option<SemanticTokensRangeResult>>(
        "semanticTokens/range",
        s.req("textDocument/semanticTokens/range", json!({"range": r})),
    );
    check::<Option<Vec<TextEdit>>>(
        "formatting",
        s.req(
            "textDocument/formatting",
            json!({"options": {"tabSize": 2, "insertSpaces": true}}),
        ),
    );
    check::<Option<Vec<TextEdit>>>(
        "rangeFormatting",
        s.req(
            "textDocument/rangeFormatting",
            json!({"range": r, "options": {"tabSize": 2, "insertSpaces": true}}),
        ),
    );
    check::<Option<CodeActionResponse>>(
        "codeAction",
        s.actions((at(src, "add(ps", 0, 0), at(src, "\nfor i", 0, 0)), &[]),
    );
    let items = s.req(
        "textDocument/prepareCallHierarchy",
        json!({"position": at(src, "add(int", 0, 0)}),
    );
    check::<Option<Vec<CallHierarchyItem>>>("prepareCallHierarchy", items.clone());
    check::<Option<Vec<CallHierarchyIncomingCall>>>(
        "incomingCalls",
        s.c.request("callHierarchy/incomingCalls", json!({"item": items[0]})),
    );
    check::<Option<Vec<CallHierarchyOutgoingCall>>>(
        "outgoingCalls",
        s.c.request("callHierarchy/outgoingCalls", json!({"item": items[0]})),
    );
    s.finish();
}

#[test]
fn ssr_join_lines_and_pulled_diagnostics() {
    let mut s = Session::start("ssr");
    let src = PROGRAM;
    s.open(src);
    let uri = s.uri.clone();

    // structural search and replace across the workspace
    let edit = s.c.request(
        "experimental/ssr",
        json!({"query": "add($a, $b) ==>> $a + $b", "parseOnly": false}),
    );
    let edits = edit["changes"][&uri].as_array().unwrap();
    let texts: Vec<&str> = edits
        .iter()
        .map(|e| e["newText"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["n + n", "ps[0].x + 3"], "{edit}");
    // a bad rule is an error
    let bad = s.c.request(
        "experimental/ssr",
        json!({"query": "add($a) ==>> $b", "parseOnly": true}),
    );
    assert!(bad.is_null(), "{bad}");

    // join lines
    let text = "int[] xs = [\n  1,\n  2,\n]\n// one\n// two\n";
    s.change(text);
    let e = s.req(
        "experimental/joinLines",
        json!({"ranges": [{"start": pos(0, 0), "end": pos(3, 0)}]}),
    );
    let mut joined = text.to_string();
    let mut es: Vec<(usize, usize, String)> = e
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            let off = |p: &Value| {
                let (l, c) = (
                    p["line"].as_u64().unwrap() as usize,
                    p["character"].as_u64().unwrap() as usize,
                );
                text.split_inclusive('\n')
                    .take(l)
                    .map(str::len)
                    .sum::<usize>()
                    + c
            };
            (
                off(&x["range"]["start"]),
                off(&x["range"]["end"]),
                x["newText"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    es.sort_by_key(|x| std::cmp::Reverse(x.0));
    for (a, b, t) in es {
        joined.replace_range(a..b, &t);
    }
    assert_eq!(joined, "int[] xs = [1, 2]\n// one\n// two\n");
    let e = s.req(
        "experimental/joinLines",
        json!({"ranges": [{"start": pos(4, 0), "end": pos(4, 0)}]}),
    );
    assert_eq!(e[0]["newText"], " ", "{e}");
    assert_eq!(e[0]["range"]["end"], pos(5, 3), "{e}");
    s.finish();

    // a client that pulls diagnostics gets them on request instead of pushed
    let dir = std::env::temp_dir().join(format!("ncc-lsp-pull-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    let file = dir.join("main.nc");
    std::fs::write(&file, "int a = \"x\"\n").unwrap();
    let furi = format!("file://{}", file.display());
    let mut c = Client::start();
    let init = c.request(
        "initialize",
        json!({"processId": null, "rootUri": null, "capabilities": {"textDocument": {"diagnostic": {}}}}),
    );
    assert_eq!(
        init["capabilities"]["diagnosticProvider"]["identifier"], "ncc",
        "{init}"
    );
    c.notify("initialized", json!({}));
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": furi, "languageId": "nc", "version": 1, "text": "int a = \"x\"\n"}}),
    );
    let r = c.request(
        "textDocument/diagnostic",
        json!({"textDocument": {"uri": furi}}),
    );
    assert_eq!(r["kind"], "full", "{r}");
    assert_eq!(r["items"].as_array().unwrap().len(), 1, "{r}");
    c.request("shutdown", Value::Null);
    c.notify("exit", Value::Null);
    assert!(c.child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extern_completion() {
    let mut s = Session::start("extern");
    std::fs::write(s.dir.join("lib.c"), "").unwrap();
    let src = "extern \"./lib.c\" as c_lib {\n  fn input() str = \"input\"\n  fn add(int a, int b) int = \"add\"\n}\n\nstr line = c_lib.input()\n@println(line, c_lib.add(1, 2))\n";
    assert_eq!(s.open(src), Vec::<Value>::new());

    // the alias is offered; its functions are not offered unqualified
    let text = format!("{src}\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "2))\n\n", 0, 5)}),
    );
    let ls = labels(&comp);
    assert!(ls.contains(&"c_lib".into()), "{ls:?}");
    assert!(
        !ls.contains(&"input".into()) && !ls.contains(&"add".into()),
        "{ls:?}"
    );
    let c = comp
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "c_lib")
        .unwrap();
    assert_eq!(c["detail"], "extern \"./lib.c\"");

    // `c_lib.` lists its functions, with argument snippets
    let text = format!("{src}c_lib.\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "c_lib.\n", 0, 6)}),
    );
    assert_eq!(labels(&comp), ["input", "add"], "{comp}");
    assert_eq!(comp[0]["insertText"], "input()$0");
    assert_eq!(comp[1]["insertText"], "add(${1:a}, ${2:b})$0");
    assert_eq!(comp[1]["detail"], "fn c_lib.add(int a, int b) int");

    // members of what an extern function returns
    let text = format!("{src}uint n = c_lib.input().\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "input().\n", 0, 8)}),
    );
    assert_eq!(labels(&comp), ["len"], "{comp}");

    // hover and go to definition on a call
    s.change(src);
    let h = s.req(
        "textDocument/hover",
        json!({"position": at(src, "c_lib.input()\n", 0, 7)}),
    );
    assert!(
        h["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("extern fn c_lib.input() str"),
        "{h}"
    );
    let d = s.req(
        "textDocument/definition",
        json!({"position": at(src, "c_lib.input()\n", 0, 7)}),
    );
    assert_eq!(d["range"]["start"]["line"], 1, "{d}");

    s.finish();
}

#[test]
fn qualified_and_generic_completion() {
    let mut s = Session::start("qualified");
    std::fs::write(s.dir.join("libc.c"), "").unwrap();
    std::fs::write(
        s.dir.join("rational.nc"),
        "pub struct Rational {\n  int numerator\n  int denominator\n}\n\npub fn map<type I, type O>(I[] arr, (fn(I) O) f) O[] {\n  mut O[] out = []\n  for i in arr {\n    out = out <> [f(arr[i])]\n  }\n  return out\n}\n",
    )
    .unwrap();
    let src = "import { \"./rational\" as rational }\nextern \"./libc.c\" as libc {\n  fn c_exit(int code) never = \"c_exit\"\n  fn c_read(str path) str = \"c_read\"\n}\n\nfn pair(int a) (int, str) {\n  return (a, \"x\")\n}\n\nrational.Rational r = rational.Rational{.numerator = 1, .denominator = 2}\nstr[] names = rational.map<int, str>([1, 2], fn(int n) str {\n  return \"{n}\"\n})\n@println(r.numerator, names, pair(1), libc.c_read(\"x\"))\n";
    let diags = s.open(src);
    assert!(diags.iter().all(|d| d["severity"] != 1), "{diags:?}");
    let item = |comp: &Value, label: &str| -> Value {
        comp.as_array()
            .unwrap()
            .iter()
            .find(|c| c["label"] == label)
            .cloned()
            .unwrap_or_else(|| panic!("no `{label}` in {comp}"))
    };

    // extern functions and module items, by their own name, inserted with the qualifier
    let text = format!("{src}c_\n");
    s.change(&text);
    let end = at(&text, "c_\n", 0, 2);
    let comp = s.req("textDocument/completion", json!({"position": end}));
    let e = item(&comp, "libc.c_exit");
    assert_eq!(e["filterText"], "c_exit");
    assert_eq!(e["insertText"], "libc.c_exit(${1:code})$0");
    item(&comp, "libc.c_read");
    let r = item(&comp, "rational.Rational");
    assert_eq!(
        (r["filterText"].clone(), r["insertText"].clone()),
        (json!("Rational"), json!("rational.Rational"))
    );

    // a module's struct as a literal where a value is expected
    let text = format!("{src}rational.Rational q = Ratio\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "Ratio\n", 0, 5)}),
    );
    assert_eq!(
        item(&comp, "rational.Rational")["insertText"],
        "rational.Rational{.numerator = $1, .denominator = $2}$0"
    );
    let text = format!("{src}rational.Rational q = rational.\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "rational.\n", 0, 9)}),
    );
    assert_eq!(
        item(&comp, "Rational")["insertText"],
        "Rational{.numerator = $1, .denominator = $2}$0"
    );

    // builtins without `@`
    let text = format!("{src}c_\n");
    s.change(&text);
    let comp = s.req("textDocument/completion", json!({"position": end}));
    let e = item(&comp, "@eprintln");
    assert_eq!(
        (e["filterText"].clone(), e["insertText"].clone()),
        (json!("eprintln"), json!("@eprintln($0)"))
    );

    // a function returning a tuple completes with its parameters
    assert_eq!(item(&comp, "pair")["insertText"], "pair(${1:a})$0");

    // generic functions fill in their type parameters first
    let g = item(&comp, "rational.map");
    assert_eq!(
        g["insertText"],
        "rational.map<${1:I}, ${2:O}>(${3:arr}, ${4:f})$0"
    );
    assert_eq!(
        g["detail"],
        "fn map<type I, type O>(I[] arr, (fn(I) O) f) O[]"
    );

    // a function literal for a callback argument, with the generic arguments substituted
    let text = format!("{src}str[] more = rational.map<int, str>([3], \n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "[3], \n", 0, 5)}),
    );
    let f = item(&comp, "fn(int a) str {}");
    assert_eq!(f["insertText"], "fn(int ${1:a}) str {\n\t$0\n}");

    // signature help shows the instantiated signature
    let sig = s.req(
        "textDocument/signatureHelp",
        json!({"position": at(&text, "[3], \n", 0, 5)}),
    );
    assert_eq!(
        sig["signatures"][0]["label"], "fn map<int, str>(int[] arr, (fn(int) str) f) str[]",
        "{sig}"
    );
    assert_eq!(sig["activeParameter"], 1);

    // generic calls are highlighted as functions
    s.change(src);
    let t = s.req("textDocument/semanticTokens/full", json!({}));
    let data: Vec<u64> = t["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap())
        .collect();
    let (mut line, mut col) = (0, 0);
    let mut toks = Vec::new();
    for c in data.chunks(5) {
        if c[0] > 0 {
            line += c[0];
            col = c[1];
        } else {
            col += c[1];
        }
        toks.push((line, col, c[3]));
    }
    let p = at(src, "map<int, str>", 0, 0);
    let tok = toks
        .iter()
        .find(|t| t.0 == p["line"].as_u64().unwrap() && t.1 == p["character"].as_u64().unwrap())
        .unwrap_or_else(|| panic!("no token for map in {toks:?}"));
    assert_eq!(tok.2, 5, "function");

    s.finish();
}

#[test]
fn generic_constants() {
    let mut s = Session::start("constants");
    let src = "fn join<type T, uint N, uint M>(T[N] a, T[M] b) T[N + M] {\n  @println(\"{N}\")\n  return a <> b\n}\n\nint[3] xs = join<int, 1, 2>([1], [2, 3])\n@println(xs)\n";
    let diags = s.open(src);
    assert!(diags.iter().all(|d| d["severity"] != 1), "{diags:?}");

    // `N` in the body: hover shows the parameter, definition jumps to it
    let use_n = at(src, "{N}", 0, 1);
    let h = s.req("textDocument/hover", json!({"position": use_n}));
    assert!(h.to_string().contains("uint N"), "{h}");
    let d = s.req("textDocument/definition", json!({"position": use_n}));
    assert_eq!(d["range"]["start"], at(src, "N, uint M", 0, 0), "{d}");

    // completion fills in every generic parameter, constants included
    let text = format!("{src}jo\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "jo\n", 0, 2)}),
    );
    let j = comp
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "join")
        .unwrap_or_else(|| panic!("no join in {comp}"));
    assert_eq!(
        j["insertText"],
        "join<${1:T}, ${2:N}, ${3:M}>(${4:a}, ${5:b})$0"
    );

    // signature help substitutes the constants
    let text = format!("{src}_ = join<int, 2, 3>(\n");
    s.change(&text);
    let sig = s.req(
        "textDocument/signatureHelp",
        json!({"position": at(&text, "3>(\n", 0, 3)}),
    );
    assert_eq!(
        sig["signatures"][0]["label"], "fn join<int, 2, 3>(int[2] a, int[3] b) int[2 + 3]",
        "{sig}"
    );
    s.finish();
}

#[test]
fn literal_tokens() {
    let mut s = Session::start("literals");
    let src = "bool b = true\nint? x = none\nif b {\n  @println(x, false)\n}\n";
    s.open(src);
    let t = s.req("textDocument/semanticTokens/full", json!({}));
    let data: Vec<u64> = t["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap())
        .collect();
    let (mut line, mut col) = (0, 0);
    let mut toks = Vec::new();
    for c in data.chunks(5) {
        if c[0] > 0 {
            line += c[0];
            col = c[1];
        } else {
            col += c[1];
        }
        toks.push((line, col, c[3], c[4]));
    }
    let tok = |needle: &str| {
        let p = at(src, needle, 0, 0);
        let (l, c) = (
            p["line"].as_u64().unwrap(),
            p["character"].as_u64().unwrap(),
        );
        *toks
            .iter()
            .find(|t| t.0 == l && t.1 == c)
            .unwrap_or_else(|| panic!("no token for {needle} in {toks:?}"))
    };
    // token type indexes: 4 enumMember, 9 keyword, 13 boolean; modifier 4 defaultLibrary
    assert_eq!(tok("true").2, 13);
    assert_eq!(tok("false").2, 13);
    assert_eq!((tok("none").2, tok("none").3), (4, 4));
    assert_eq!(tok("if").2, 9);
    s.finish();
}

#[test]
fn first_argument_completion() {
    let mut s = Session::start("firstarg");
    std::fs::write(s.dir.join("lib.c"), "").unwrap();
    std::fs::write(
        s.dir.join("seq.nc"),
        "pub fn first<type T>(T[] arr) T {\n  return arr[0]\n}\n\nfn hidden(int[] _arr) {}\n",
    )
    .unwrap();
    let src = "import { \"./seq\" as seq }\nextern \"./lib.c\" as lib {\n  fn c_abs(int x) int = \"c_abs\"\n}\n\n/// Adds two numbers.\nfn add(int a, int b) int {\n  return a + b\n}\n\nfn shout(str s) str {\n  return s\n}\n\nint n = add(1, 2)\nint[] xs = [n, 2, 3]\nint[3] fixed = [1, 2, 3]\n@println(shout(\"x\"), lib.c_abs(n), seq.first<int>(xs), fixed)\n";
    let diags = s.open(src);
    assert!(diags.iter().all(|d| d["severity"] != 1), "{diags:?}");
    let find = |comp: &Value, label: &str| -> Option<Value> {
        comp.as_array()
            .unwrap()
            .iter()
            .find(|c| c["label"] == label)
            .cloned()
    };

    // `n.` offers functions taking an `int` first, rewritten as calls
    let text = format!("{src}n.\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "n.\n", 0, 2)}),
    );
    let add = find(&comp, "add").unwrap_or_else(|| panic!("no add in {comp}"));
    assert_eq!(add["textEdit"]["newText"], "add(n, ${1:b})$0");
    assert_eq!(add["filterText"], "n.add");
    assert_eq!(add["textEdit"]["range"]["start"], at(&text, "n.\n", 0, 0));
    assert!(
        add["documentation"]
            .to_string()
            .contains("Adds two numbers")
    );
    let abs = find(&comp, "lib.c_abs").unwrap_or_else(|| panic!("no lib.c_abs in {comp}"));
    assert_eq!(abs["textEdit"]["newText"], "lib.c_abs(n)$0");
    assert!(find(&comp, "shout").is_none(), "{comp}");
    assert!(find(&comp, "seq.first").is_none(), "{comp}");

    // generic functions get their type arguments from the receiver; private functions of
    // other modules are left out
    let text = format!("{src}xs.\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "xs.\n", 0, 3)}),
    );
    let first = find(&comp, "seq.first").unwrap_or_else(|| panic!("no seq.first in {comp}"));
    assert_eq!(first["textEdit"]["newText"], "seq.first<int>(xs)$0");
    assert!(find(&comp, "seq.hidden").is_none(), "{comp}");
    assert!(find(&comp, "len").is_some(), "{comp}");
    assert!(find(&comp, "add").is_none(), "{comp}");

    // a fixed-size array doesn't convert to a dynamic one implicitly
    let text = format!("{src}fixed.\n");
    s.change(&text);
    let comp = s.req(
        "textDocument/completion",
        json!({"position": at(&text, "fixed.\n", 0, 6)}),
    );
    assert!(find(&comp, "seq.first").is_none(), "{comp}");
    s.finish();
}

#[test]
fn generic_bodies() {
    let mut s = Session::start("genericbody");
    let src = "fn get_x<type T>(T val, int offset) int {\n  int base = offset * 2\n  return val.x + base\n}\n\nfn rest<type T, uint N>(T[N] arr) T[N - 1] {\n  T first = arr[0]\n  T[N - 1] tail = arr[1:$]\n  return tail\n}\n";
    let diags = s.open(src);
    // only the unused-function warnings: nothing from checking the bodies with placeholders
    assert!(
        diags.iter().all(|d| d["message"]
            .as_str()
            .unwrap()
            .starts_with("unused function")),
        "{diags:?}"
    );
    let hover = |s: &mut Session, needle: &str, nth: usize| {
        s.req(
            "textDocument/hover",
            json!({"position": at(src, needle, nth, 0)}),
        )["contents"]["value"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    // locals and parameters of a generic function that is never called
    assert!(hover(&mut s, "base\n}", 0).contains("int base"));
    assert!(hover(&mut s, "first = ", 0).contains("T first"));
    assert!(hover(&mut s, "tail\n}", 0).contains("T[N - 1] tail"));
    let d = s.req(
        "textDocument/definition",
        json!({"position": at(src, "offset * 2", 0, 0)}),
    );
    assert_eq!(d["range"]["start"], at(src, "offset) int", 0, 0), "{d}");

    // completion inside the body, including members of values whose type is known
    let text = src.replace("  return tail\n", "  return tail\n  ta\n  arr.\n");
    s.change(&text);
    let labels = |s: &mut Session, needle: &str, delta: usize| -> Vec<String> {
        let comp = s.req(
            "textDocument/completion",
            json!({"position": at(&text, needle, 0, delta)}),
        );
        comp.as_array()
            .unwrap()
            .iter()
            .map(|c| c["label"].as_str().unwrap().to_string())
            .collect()
    };
    let l = labels(&mut s, "ta\n", 2);
    assert!(
        ["tail", "first", "arr"]
            .iter()
            .all(|n| l.contains(&n.to_string())),
        "{l:?}"
    );
    assert!(labels(&mut s, "arr.\n", 4).contains(&"len".to_string()));
    s.finish();
}

#[test]
fn function_value_completion() {
    let mut s = Session::start("fnvalue");
    let src = "fn func() int {\n  return 1\n}\n\nfn other_func((fn() int) f) int {\n  return f()\n}\n\nstruct Handler {\n  (fn() int) cb\n  int n\n}\n\nstruct Box<type T> {\n  T value\n  (fn() int) cb\n}\n\nfn pick<type T>(T a, T b) T {\n  return a\n}\n\nfn map<type I, type O>(I[] xs, (fn(I) O) f) O[] {\n  mut O[] out = []\n  for i in xs {\n    out = out <> [f(xs[i])]\n  }\n  return out\n}\n\nmut (fn() int) g = func\nint v = other_func(func)\nmut Handler h = Handler{.cb = func, .n = 1}\nHandler[] hs = [h]\n@println(v, g(), h.n, hs.len)\n";
    let diags = s.open(src);
    assert!(diags.iter().all(|d| d["severity"] != 1), "{diags:?}");
    // the insert text of `label` when completing at the end of `line` appended to `src`
    let mut insert = |line: &str, label: &str| -> Value {
        let text = format!("{src}{line}\n");
        s.change(&text);
        let comp = s.req(
            "textDocument/completion",
            json!({"position": at(&text, &format!("{line}\n"), 0, line.len())}),
        );
        comp.as_array()
            .unwrap()
            .iter()
            .find(|c| c["label"] == label)
            .unwrap_or_else(|| panic!("no `{label}` in {comp}"))["insertText"]
            .clone()
    };

    // a call where a value is expected: the cursor goes inside the parentheses only when
    // there are arguments
    assert_eq!(insert("int x = f", "func"), "func()$0");
    assert_eq!(insert("int x = o", "other_func"), "other_func(${1:f})$0");
    assert_eq!(insert("int x = e", "@eprintln"), "@eprintln($0)");
    assert_eq!(insert("str[] a = a", "@args"), "@args()$0");
    assert_eq!(insert("str[] a = @a", "args"), "args()$0");
    assert_eq!(
        insert("int x = 1 + other_func(func) + f", "func"),
        "func()$0"
    );
    assert_eq!(insert("@println(f", "func"), "func()$0");

    // the bare function where a function value is expected
    assert_eq!(insert("int x = other_func(f", "func"), Value::Null);
    assert_eq!(insert("int x = other_func(a", "@args"), "@args");
    assert_eq!(insert("(fn() int) h = f", "func"), Value::Null);
    assert_eq!(insert("mut (fn() int) h = f", "func"), Value::Null);
    assert_eq!(insert("g = f", "func"), Value::Null);
    assert_eq!(
        insert("fn make() (fn() int) {\n  return f", "func"),
        Value::Null
    );
    assert_eq!(insert("fn make() int {\n  return f", "func"), "func()$0");
    assert_eq!(
        insert("fn make() (fn() int) {\n  if true {\n    return f", "func"),
        Value::Null
    );

    // struct literal fields and member assignments
    assert_eq!(insert("Handler k = Handler{.cb = f", "func"), Value::Null);
    assert_eq!(insert("Handler k = Handler{.n = f", "func"), "func()$0");
    assert_eq!(
        insert("Handler k = Handler{.n = 1, .cb = f", "func"),
        Value::Null
    );
    assert_eq!(insert("h.cb = f", "func"), Value::Null);
    assert_eq!(insert("hs[0].cb = f", "func"), Value::Null);
    assert_eq!(insert("h.n = f", "func"), "func()$0");

    // structs complete to a literal where a value is expected
    let literal = "Handler{.cb = $1, .n = $2}$0";
    assert_eq!(insert("Handler k = H", "Handler"), literal);
    assert_eq!(insert("@println(H", "Handler"), literal);
    assert_eq!(insert("Handler[] ks = [H", "Handler"), literal);
    assert_eq!(insert("fn mk() Handler {\n  return H", "Handler"), literal);

    // generic structs take their type arguments from the expected type, or placeholders
    assert_eq!(
        insert("Box<int> b = B", "Box"),
        "Box<int>{.value = $1, .cb = $2}$0"
    );
    assert_eq!(
        insert("@println(B", "Box"),
        "Box<${1:T}>{.value = $2, .cb = $3}$0"
    );
    assert_eq!(insert("Box<int> b = Box<int>{.cb = f", "func"), Value::Null);
    assert_eq!(insert("Box<int> b = Box<int>{.", "value"), "value = ");

    // tuple elements
    assert_eq!(insert("(int, Handler) t = (1, H", "Handler"), literal);
    assert_eq!(insert("(int, (fn() int)) t = (1, f", "func"), Value::Null);
    assert_eq!(insert("(int, (fn() int)) t = (f", "func"), "func()$0");
    assert_eq!(insert("int x = other_func((f", "func"), Value::Null);
    assert_eq!(insert("(int, H", "Handler"), Value::Null);

    // generic parameters resolved from the arguments before
    assert_eq!(insert("Handler k = pick(h, H", "Handler"), literal);
    assert_eq!(
        insert(
            "Box<int> k = pick(Box<int>{.value = 1, .cb = func}, B",
            "Box"
        ),
        "Box<int>{.value = $1, .cb = $2}$0"
    );
    assert_eq!(
        insert(
            "Box<int>[] k = map([1], fn(int n) Box<int> {\n  return B",
            "Box"
        ),
        "Box<int>{.value = $1, .cb = $2}$0"
    );
    assert_eq!(insert("(fn() int) k = pick(func, f", "func"), Value::Null);
    assert_eq!(insert("int k = pick(1, f", "func"), "func()$0");
    assert_eq!(insert("int k = pick(f", "func"), "func()$0");
    assert_eq!(insert("Handler[] k = pick([h], [H", "Handler"), literal);
    assert_eq!(insert("int[] k = map([1, 2], f", "func"), Value::Null);
    assert_eq!(insert("Handler[] k = map([h], f", "func"), Value::Null);

    // array elements
    assert_eq!(insert("Handler[] ks = [h, H", "Handler"), literal);
    assert_eq!(insert("(fn() int)[] fs = [f", "func"), Value::Null);
    assert_eq!(insert("(fn() int)[] fs = [func, f", "func"), Value::Null);
    assert_eq!(insert("int[] xs = [1, f", "func"), "func()$0");
    assert_eq!(insert("@println(hs[f", "func"), "func()$0");

    // but not where a type is expected
    assert_eq!(insert("H", "Handler"), Value::Null);
    assert_eq!(insert("fn take(int a, H", "Handler"), Value::Null);
    assert_eq!(insert("(fn(H", "Handler"), Value::Null);

    s.finish();
}
