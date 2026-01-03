.PHONY: sync-example sync-example-init sync-example-terminate docker-up docker-down

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
