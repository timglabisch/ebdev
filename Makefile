.PHONY: sync-example sync-example-init sync-example-terminate docker-up docker-down build-linux build-linux-arm64 build-linux-all build test-docker test-docker-smoke test-taskrunner test-integration test-integration-rust test-wasm test-wasm-integration

# =============================================================================
# Linux Bridge Builds (statisch gelinkt mit musl, ~1MB)
# =============================================================================

# Build Linux x86_64 bridge binary
build-linux:
	@mkdir -p target/linux
	docker buildx build --platform linux/amd64 -f Dockerfile.build --target export --output type=local,dest=target/linux .
	@echo "Binary: target/linux/ebdev-bridge (x86_64)"
	@ls -lh target/linux/ebdev-bridge

# Build Linux ARM64 bridge binary
build-linux-arm64:
	@mkdir -p target/linux-arm64
	docker buildx build --platform linux/arm64 -f Dockerfile.build --target export --output type=local,dest=target/linux-arm64 .
	@echo "Binary: target/linux-arm64/ebdev-bridge (aarch64)"
	@ls -lh target/linux-arm64/ebdev-bridge

# Build all Linux bridge binaries
build-linux-all: build-linux build-linux-arm64

# =============================================================================
# Main Build
# =============================================================================

# Build release binary with embedded Linux bridge (x86_64)
build: build-linux
	cargo build --release --package ebdev
	@echo "Binary: target/release/ebdev (with embedded Linux bridge)"
	@ls -lh target/release/ebdev

# Start Docker and run sync (final stage only)
sync-example: docker-up
	@sleep 2
	cd example && cargo run --release --package ebdev -- mutagen sync

# Start Docker and run full sync from scratch (terminate all, run all stages)
sync-example-init: docker-up
	@sleep 2
	cd example && cargo run --release --package ebdev -- mutagen sync --init --sync

# Terminate all project sessions
sync-example-terminate:
	cd example && cargo run --release --package ebdev -- mutagen sync --terminate

docker-up:
	cd example && docker compose up -d --build

docker-down:
	cd example && docker compose down

# =============================================================================
# Task Runner Tests
# =============================================================================

# Run full docker integration test (starts docker if needed)
test-docker: build-linux docker-up
	@sleep 1
	cd example && cargo run --package ebdev -- task test_docker

# Quick docker smoke test
test-docker-smoke: build-linux docker-up
	@sleep 1
	cd example && cargo run --package ebdev -- task test_docker_smoke

# =============================================================================
# Integration Tests
# =============================================================================

# Run all integration tests (downloads node, pnpm, mutagen, rust)
test-integration:
	cargo test --package ebdev --test integration_test -- --test-threads=1

# Run only rust-related integration tests
test-integration-rust:
	cargo test --package ebdev --test integration_test test_rust -- --test-threads=1

# Run all local taskrunner tests (no docker needed)
test-taskrunner:
	cd example && cargo run --package ebdev -- task test_smoke
	cd example && cargo run --package ebdev -- task test_stages
	cd example && cargo run --package ebdev -- task test_try

# =============================================================================
# WASM Tests
# =============================================================================

# Run WASM unit tests
test-wasm:
	cargo test --package ebdev_remote --features wasm-runtime -- wasm

# Run WASM integration test (requires rust toolchain for .rs compilation)
test-wasm-integration:
	cd example && cargo run --package ebdev -- task test_wasm
