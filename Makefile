.PHONY: build install install-global update test check fmt clippy clean build-web build-web-dev wasm-target build-windows coverage dev run

# Build and run koji (frontend + backend)
run: build
	cargo run --release --bin koji service-run --proxy

# Run Leptos frontend dev server with hot reload on http://localhost:8080
dev: wasm-target
	cd crates/koji-web && trunk serve --port 8080

# Ensure the wasm32 target is installed (idempotent — safe to run multiple times)
wasm-target:
	rustup target add wasm32-unknown-unknown

# Build the Leptos WASM frontend into crates/koji-web/dist/ (required before any Rust release build)
build-frontend: wasm-target
	cd crates/koji-web && trunk build --release

# Development WASM build (unoptimised, faster iteration)
build-frontend-dev: wasm-target
	cd crates/koji-web && trunk build

# Full release build: frontend first, then the Rust workspace
build: build-frontend
	cargo build --release --workspace

# Install koji CLI (includes web UI via default feature)
install: build-frontend
	cargo install --path crates/koji-cli --force

# Stop service, rebuild + reinstall (frontend + backend), restart service
update: build-frontend
	koji service stop || true
	cargo build --release --workspace
	cargo install --path crates/koji-cli --force
	koji service start

# Windows: copy release binary to Program Files (requires admin)
install-global: build
	copy target\release\koji.exe "C:\Program Files\Koji\koji.exe"

# Run all tests including the koji-web SSR integration tests
test: build-frontend-dev
	cargo test --workspace
	cargo test --package koji-web --features ssr

check: fmt-check clippy test build-windows

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

# Lint everything including the server-side koji-web code
clippy:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy --package koji-web --features ssr -- -D warnings

clean:
	cargo clean
	rm -rf crates/koji-web/dist

# Aliases kept for backwards compat — both now build the main koji binary
build-web: build

build-web-dev: build-frontend-dev
	cargo build --workspace

# Cross-compile to Windows from Linux (requires mingw64-gcc-c++)
build-windows:
	cargo build --target x86_64-pc-windows-gnu --release

# Run code coverage analysis with cargo-tarpaulin (HTML report in target/coverage/)
coverage:
	cargo tarpaulin --workspace --features ssr --out Html --output-dir target/coverage --timeout 300
