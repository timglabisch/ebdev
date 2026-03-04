.PHONY: help tasks hello greet info parallel shell build \
        complex smoke stages timeout fail try \
        hello-headless parallel-headless complex-headless smoke-headless \
        complex-debug smoke-debug

EBDEV := ../target/debug/ebdev

help:
	@echo "Available targets:"
	@echo ""
	@echo "  Basic:"
	@echo "    make tasks        - List all available tasks"
	@echo "    make build        - Build ebdev first, then run hello"
	@echo ""
	@echo "  Simple Tasks (TUI by default):"
	@echo "    make hello        - Run hello task"
	@echo "    make greet        - Run greet task"
	@echo "    make info         - Run info task"
	@echo "    make shell        - Run shell test"
	@echo ""
	@echo "  Parallel:"
	@echo "    make parallel     - Run parallel test"
	@echo ""
	@echo "  Integration Tests:"
	@echo "    make complex      - Run complex integration test"
	@echo "    make smoke        - Run quick smoke test"
	@echo ""
	@echo "  Headless (--no-tui):"
	@echo "    make hello-headless    - Run hello task headless"
	@echo "    make parallel-headless - Run parallel test headless"
	@echo "    make complex-headless  - Run complex integration test headless"
	@echo "    make smoke-headless    - Run quick smoke test headless"
	@echo ""
	@echo "  Feature Tests:"
	@echo "    make stages       - Test stage functionality"
	@echo "    make timeout      - Test timeout handling"
	@echo "    make fail         - Test error handling (will fail)"
	@echo "    make try          - Test tryExec (errors ignored)"
	@echo ""
	@echo "  Debug (with JSON logging):"
	@echo "    make complex-debug - TUI with debug log to /tmp/ebdev-debug.json"
	@echo "    make smoke-debug   - TUI with debug log to /tmp/ebdev-debug.json"

tasks:
	$(EBDEV) tasks

# Simple tasks (TUI by default)
hello:
	$(EBDEV) task hello

greet:
	$(EBDEV) task greet

info:
	$(EBDEV) task info

shell:
	$(EBDEV) task test_shell

# Parallel
parallel:
	$(EBDEV) task test_parallel

# Integration tests
complex:
	$(EBDEV) task test_complex

smoke:
	$(EBDEV) task test_smoke

# Headless variants
hello-headless:
	$(EBDEV) task hello --no-tui

parallel-headless:
	$(EBDEV) task test_parallel --no-tui

complex-headless:
	$(EBDEV) task test_complex --no-tui

smoke-headless:
	$(EBDEV) task test_smoke --no-tui

# Feature tests
stages:
	$(EBDEV) task test_stages

timeout:
	$(EBDEV) task test_timeout

fail:
	$(EBDEV) task test_fail

try:
	$(EBDEV) task test_try

# Debug (with --debug-log)
complex-debug:
	$(EBDEV) task test_complex --debug-log /tmp/ebdev-debug.json
	@echo ""
	@echo "Debug log written to /tmp/ebdev-debug.json"

smoke-debug:
	$(EBDEV) task test_smoke --debug-log /tmp/ebdev-debug.json
	@echo ""
	@echo "Debug log written to /tmp/ebdev-debug.json"

# Build
build:
	cd .. && cargo build -p ebdev
	$(EBDEV) task hello
