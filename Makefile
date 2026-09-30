build:
	@cargo build --release

install: build
	@install target/release/ncc $(HOME)/.local/bin/ncc

test: build
	@cargo test
	@cargo clippy --all-targets -- -D warnings

.PHONY: grammar-test
grammar-test:
	@python3 tree-sitter-nc/scripts/test.py
