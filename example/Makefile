.PHONY: help tasks hello greet info parallel shell build parallel-tui hello-tui

EBDEV := ../target/debug/ebdev

help:
	@echo "Available targets:"
	@echo "  make tasks        - List all available tasks"
	@echo "  make hello        - Run hello task (headless)"
	@echo "  make hello-tui    - Run hello task (TUI)"
	@echo "  make greet        - Run greet task"
	@echo "  make info         - Run info task"
	@echo "  make parallel     - Run parallel test (headless)"
	@echo "  make parallel-tui - Run parallel test (TUI)"
	@echo "  make shell        - Run shell test"
	@echo "  make build        - Build ebdev first, then run hello"

tasks:
	$(EBDEV) tasks

hello:
	$(EBDEV) task hello

hello-tui:
	$(EBDEV) task hello --tui

greet:
	$(EBDEV) task greet

info:
	$(EBDEV) task info

parallel:
	$(EBDEV) task test_parallel

parallel-tui:
	$(EBDEV) task test_parallel --tui

shell:
	$(EBDEV) task test_shell

build:
	cd .. && cargo build -p ebdev
	$(EBDEV) task hello
