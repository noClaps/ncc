build:
	@cargo build --release

install: build
	@install target/release/ncc $(HOME)/.local/bin/ncc

grammar:
	@mkdir -p target
	@cc -O2 -shared -fPIC -I tree-sitter-nc/src tree-sitter-nc/src/parser.c tree-sitter-nc/src/scanner.c -o target/nc.so

grammar-test:
	@cd tree-sitter-nc && tree-sitter test

test: build
	@cargo test
	@cargo clippy --all-targets -- -D warnings
