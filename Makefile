build:
	@cargo build --release

install: build
	@install target/release/ncc $(HOME)/.local/bin/ncc

test: build
	@cargo test
	@cargo clippy --all-targets -- -D warnings
