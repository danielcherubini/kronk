.PHONY: build install install-global update test check fmt clippy clean build-web build-web-dev wasm-target

# Ensure the wasm32 target is installed (idempotent — safe to run multiple times)
wasm-target:
	rustup target add wasm32-unknown-unknown

# Build the Leptos WASM frontend into crates/kronk-web/dist/ (required before any Rust release build)
build-frontend: wasm-target
	cd crates/kronk-web && trunk build --release

# Development WASM build (unoptimised, faster iteration)
build-frontend-dev: wasm-target
	cd crates/kronk-web && trunk build

# Full release build: frontend first, then the Rust workspace
build: build-frontend
	cargo build --release --workspace

# Install kronk CLI (includes web UI via default feature)
install: build-frontend
	cargo install --path crates/kronk-cli --force

# Stop service, rebuild + reinstall, restart service
update: build
	kronk service stop || true
	cargo install --path crates/kronk-cli --force
	kronk service start

# Windows: copy release binary to Program Files (requires admin)
install-global: build
	copy target\release\kronk.exe "C:\Program Files\Kronk\kronk.exe"

# Run all tests including the kronk-web SSR integration tests
test: build-frontend-dev
	cargo test --workspace
	cargo test --package kronk-web --features ssr

check: fmt clippy test

fmt:
	cargo fmt --all

# Lint everything including the server-side kronk-web code
clippy:
	cargo clippy --workspace -- -D warnings
	cargo clippy --package kronk-web --features ssr -- -D warnings

clean:
	cargo clean
	rm -rf crates/kronk-web/dist

# Aliases kept for backwards compat — both now build the main kronk binary
build-web: build

build-web-dev: build-frontend-dev
	cargo build --workspace
