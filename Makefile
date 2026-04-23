.PHONY: build install install-global update test check fmt clippy clean build-web build-web-dev wasm-target build-windows coverage dev run

# Build and run tama (frontend + backend)
run: build
	cargo run --release --bin tama service-run --proxy

# Run Leptos frontend dev server with hot reload on http://localhost:8080
dev: wasm-target
	cd crates/tama-web && trunk serve --port 8080

# Ensure the wasm32 target is installed (idempotent — safe to run multiple times)
wasm-target:
	rustup target add wasm32-unknown-unknown

# Build the Leptos WASM frontend into crates/tama-web/dist/ (required before any Rust release build)
build-frontend: wasm-target
	cd crates/tama-web && trunk build --release

# Development WASM build (unoptimised, faster iteration)
build-frontend-dev: wasm-target
	cd crates/tama-web && trunk build

# Full release build: frontend first, then the Rust workspace
build: build-frontend
	cargo build --release --workspace

# Install tama CLI (includes web UI via default feature)
install: build-frontend
	cargo install --path crates/tama-cli --force

# Stop service, rebuild + reinstall (frontend + backend), restart service
update: build-frontend
	cargo build --release --workspace
	tama service stop || true
	cargo install --path crates/tama-cli --force
	tama service start

# Windows: copy release binary to Program Files (requires admin)
install-global: build
	copy target\release\tama.exe "C:\Program Files\Tama\tama.exe"

# Run all tests including the tama-web SSR integration tests
test: build-frontend-dev
	cargo test --workspace
	cargo test --package tama-web --features ssr

check: fmt-check clippy test build-windows

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

# Lint everything including the server-side tama-web code
clippy:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy --package tama-web --features ssr -- -D warnings

clean:
	cargo clean
	rm -rf crates/tama-web/dist

# Aliases kept for backwards compat — both now build the main tama binary
build-web: build

build-web-dev: build-frontend-dev
	cargo build --workspace

# Cross-compile to Windows from Linux (requires mingw64-gcc-c++)
build-windows:
	cargo build --target x86_64-pc-windows-gnu --release

# Run code coverage analysis with cargo-tarpaulin (HTML report in target/coverage/)
coverage:
	cargo tarpaulin --workspace --features ssr --out Html --output-dir target/coverage --timeout 300
