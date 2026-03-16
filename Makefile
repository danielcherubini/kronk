.PHONY: build install install-global test check fmt clippy clean

build:
	cargo build --release --workspace

install:
	cargo install --path crates/kronk-cli --force

# Windows: copy release binary to Program Files (requires admin)
install-global: build
	copy target\release\kronk.exe "C:\Program Files\Kronk\kronk.exe"

test:
	cargo test --workspace

check: fmt clippy test

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace -- -D warnings

clean:
	cargo clean
