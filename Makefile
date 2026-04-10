.PHONY: build install install-global update test check fmt clippy clean build-web build-web-dev wasm-target

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

# Stop service, rebuild + reinstall, restart service
update: build
	koji service stop || true
	cargo install --path crates/koji-cli --force
	koji service start

# Windows: copy release binary to Program Files (requires admin)
install-global: build
	copy target\release\koji.exe "C:\Program Files\Koji\koji.exe"

# Run all tests including the koji-web SSR integration tests
test: build-frontend-dev
	cargo test --workspace
	cargo test --package koji-web --features ssr

check: fmt-check clippy test

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

# Lint everything including the server-side koji-web code
clippy:
	cargo clippy --workspace -- -D warnings
	cargo clippy --package koji-web --features ssr -- -D warnings

clean:
	cargo clean
	rm -rf crates/koji-web/dist

# Aliases kept for backwards compat — both now build the main koji binary
build-web: build

build-web-dev: build-frontend-dev
	cargo build --workspace
