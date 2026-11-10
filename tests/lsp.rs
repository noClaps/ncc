//! Drives `ncc lsp` over stdio and checks diagnostics, hover, definition, symbols and completion.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

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
        Client { child, stdin, stdout, next_id: 1 }
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
    let init = c.request("initialize", json!({"processId": null, "rootUri": null, "capabilities": {}}));
    assert!(init["capabilities"]["hoverProvider"].as_bool().unwrap());
    c.notify("initialized", json!({}));

    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": "nc", "version": 1, "text": src}}),
    );
    assert!(c.diagnostics(&uri).is_empty(), "expected no diagnostics");

    // hover on `total` in `@println(total)`
    let h = c.request("textDocument/hover", json!({"textDocument": {"uri": uri}, "position": pos(11, 10)}));
    assert!(h["contents"]["value"].as_str().unwrap().contains("int total"), "{h}");

    // hover on the call `add`
    let h = c.request("textDocument/hover", json!({"textDocument": {"uri": uri}, "position": pos(10, 13)}));
    assert!(h["contents"]["value"].as_str().unwrap().contains("fn add(int, int) int"), "{h}");

    // go to definition of `add`
    let d = c.request("textDocument/definition", json!({"textDocument": {"uri": uri}, "position": pos(10, 13)}));
    assert_eq!(d["range"]["start"]["line"], 5, "{d}");

    // go to definition of field `x` in `p.x`
    let d = c.request("textDocument/definition", json!({"textDocument": {"uri": uri}, "position": pos(10, 18)}));
    assert_eq!(d["range"]["start"]["line"], 1, "{d}");

    // document symbols
    let s = c.request("textDocument/documentSymbol", json!({"textDocument": {"uri": uri}}));
    let names: Vec<&str> = s.as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Point", "add", "p", "total"]);

    // member completion after `p.`
    let edited = format!("{src}int z = p.\n");
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": edited}]}),
    );
    let diags = c.diagnostics(&uri);
    assert_eq!(diags.len(), 1, "expected a parse error: {diags:?}");
    let comp = c.request("textDocument/completion", json!({"textDocument": {"uri": uri}, "position": pos(12, 10)}));
    let labels: Vec<&str> = comp.as_array().unwrap().iter().map(|x| x["label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["x", "y"], "{comp}");

    // general completion includes keywords and declarations
    let comp = c.request("textDocument/completion", json!({"textDocument": {"uri": uri}, "position": pos(12, 0)}));
    let labels: Vec<&str> = comp.as_array().unwrap().iter().map(|x| x["label"].as_str().unwrap()).collect();
    assert!(labels.contains(&"while") && labels.contains(&"add") && labels.contains(&"total"), "{labels:?}");

    // type error diagnostic with position
    let bad = "int a = \"oops\"\n";
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 3}, "contentChanges": [{"text": bad}]}),
    );
    let diags = c.diagnostics(&uri);
    assert_eq!(diags.len(), 1);
    assert!(diags[0]["message"].as_str().unwrap().contains("expected `int`, found `str`"));
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
