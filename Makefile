build:
	@cargo build --release

install: build
	@install target/release/ncc $(HOME)/.local/bin/ncc

zed: build
	@rustc --edition=2024 scripts/prepare-zed.rs -o target/prepare-zed
	@target/prepare-zed

test: build
	@cargo test
	@cargo clippy --all-targets -- -D warnings
