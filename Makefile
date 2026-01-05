.PHONY: sync-example sync-example-init sync-example-terminate docker-up docker-down build-linux

# Build Linux bridge binary via Docker (statisch gelinkt, ~1MB)
build-linux:
	@mkdir -p target/linux
	docker build -f Dockerfile.build --target export --output type=local,dest=target/linux .
	@echo "Binary: target/linux/ebdev-bridge"
	@ls -lh target/linux/ebdev-bridge

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
