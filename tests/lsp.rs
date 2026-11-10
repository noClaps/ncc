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
    fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to start ncc lsp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client {
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
    at.sort();
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
    let col = off - src[..off].rfind('\n').map(|i| i + 1).unwrap_or(0);
    pos(line as u32, col as u32)
}

fn labels(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
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
    fn start(name: &str) -> Session {
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
            json!({"processId": null, "rootUri": root, "capabilities": {
                "textDocument": {"completion": {"completionItem": {"snippetSupport": true}}}
            }}),
        );
        c.notify("initialized", json!({}));
        Session {
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
        self.c.diagnostics(&self.uri.clone())
    }

    fn change(&mut self, text: &str) -> Vec<Value> {
        self.version += 1;
        self.c.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": self.uri, "version": self.version}, "contentChanges": [{"text": text}]}),
        );
        self.c.diagnostics(&self.uri.clone())
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
    let text = src.replace(", util.helper()", "") + "fn f(int unused) {\n  int also = 1\n}\n";
    let diags = s.change(&text);
    let mut msgs: Vec<&str> = diags
        .iter()
        .map(|d| d["message"].as_str().unwrap())
        .collect();
    msgs.sort();
    assert_eq!(
        msgs,
        [
            "unused import `util`",
            "unused parameter `unused`",
            "unused variable `also`"
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
        .find(|d| d["message"] == "unused variable `also`")
        .unwrap()
        .clone();
    let acts = s.actions(
        (d["range"]["start"].clone(), d["range"]["end"].clone()),
        std::slice::from_ref(&d),
    );
    assert_eq!(titles(&acts)[0], "Rename to `_`", "{acts}");
    assert_eq!(acts[0]["edit"]["changes"][&s.uri][0]["range"], d["range"]);
    let d = diags
        .iter()
        .find(|d| d["message"] == "unused import `util`")
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
    let acts = s.actions((a.clone(), b.clone()), &[]);
    let ex = acts
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["title"] == "Extract into a variable")
        .unwrap_or_else(|| panic!("{acts}"));
    let edits = ex["edit"]["changes"][&s.uri].as_array().unwrap();
    assert_eq!(edits[0]["newText"], "int value = add(ps[0].x, 3)\n");
    assert_eq!(edits[1]["newText"], "value");

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
    let uri = s.uri.clone();
    assert_eq!(s.c.diagnostics(&uri), Vec::<Value>::new());
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
