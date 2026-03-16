.PHONY: build install test check fmt clippy clean

build:
	cargo build --release --workspace

install:
	cargo install --path crates/kronk-cli --force
	cargo install --path crates/kronk-mock --force

test:
	cargo test --workspace

check: fmt clippy test

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace -- -D warnings

clean:
	cargo clean
