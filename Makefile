.PHONY: build install update test check fmt clippy clean build-web build-web-dev wasm-target coverage dev run

# Run tama in dev mode: proxy (:11434) + web UI (:11435)
# Proxy runs in background, web in foreground — Ctrl+C stops web, `make stop` stops proxy.
run: build-frontend-dev
	@echo "Starting tama proxy on :11434..."
	@cargo run --bin tama serve --port 11434 &
	@sleep 1
	@echo "Starting web UI on :11435..."
	@cargo run --bin tama web --port 11435 --proxy-url http://127.0.0.1:11434

# Stop the background proxy process started by `make run`
stop:
	@pkill -f "cargo run --bin tama serve" 2>/dev/null || true
	@echo "Stopped tama proxy"

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

# Run all tests including the tama-web SSR integration tests
test: build-frontend-dev
	cargo test --workspace
	cargo test --package tama-web --features ssr

check: fmt-check clippy test

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

# Run code coverage analysis with cargo-tarpaulin (HTML report in target/coverage/)
coverage:
	cargo tarpaulin --workspace --features ssr --out Html --output-dir target/coverage --timeout 300
