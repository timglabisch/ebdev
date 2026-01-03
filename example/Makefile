.PHONY: help tasks hello greet info parallel shell build

EBDEV := ../target/debug/ebdev

help:
	@echo "Available targets:"
	@echo "  make tasks    - List all available tasks"
	@echo "  make hello    - Run hello task"
	@echo "  make greet    - Run greet task"
	@echo "  make info     - Run info task"
	@echo "  make parallel - Run parallel test"
	@echo "  make shell    - Run shell test"
	@echo "  make build    - Build ebdev first, then run hello"

tasks:
	$(EBDEV) tasks

hello:
	$(EBDEV) task hello

greet:
	$(EBDEV) task greet

info:
	$(EBDEV) task info

parallel:
	$(EBDEV) task test_parallel

shell:
	$(EBDEV) task test_shell

build:
	cd .. && cargo build -p ebdev
	$(EBDEV) task hello
