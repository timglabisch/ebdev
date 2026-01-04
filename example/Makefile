.PHONY: help tasks hello greet info parallel shell build parallel-tui hello-tui \
        complex complex-tui smoke smoke-tui stages timeout fail try \
        complex-debug smoke-debug

EBDEV := ../target/debug/ebdev

help:
	@echo "Available targets:"
	@echo ""
	@echo "  Basic:"
	@echo "    make tasks        - List all available tasks"
	@echo "    make build        - Build ebdev first, then run hello"
	@echo ""
	@echo "  Simple Tasks:"
	@echo "    make hello        - Run hello task (headless)"
	@echo "    make hello-tui    - Run hello task (TUI)"
	@echo "    make greet        - Run greet task"
	@echo "    make info         - Run info task"
	@echo "    make shell        - Run shell test"
	@echo ""
	@echo "  Parallel:"
	@echo "    make parallel     - Run parallel test (headless)"
	@echo "    make parallel-tui - Run parallel test (TUI)"
	@echo ""
	@echo "  Integration Tests:"
	@echo "    make complex      - Run complex integration test (headless)"
	@echo "    make complex-tui  - Run complex integration test (TUI)"
	@echo "    make smoke        - Run quick smoke test"
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

# Simple tasks
hello:
	$(EBDEV) task hello

hello-tui:
	$(EBDEV) task hello --tui

greet:
	$(EBDEV) task greet

info:
	$(EBDEV) task info

shell:
	$(EBDEV) task test_shell

# Parallel
parallel:
	$(EBDEV) task test_parallel

parallel-tui:
	$(EBDEV) task test_parallel --tui

# Integration tests
complex:
	$(EBDEV) task test_complex

complex-tui:
	$(EBDEV) task test_complex --tui

smoke:
	$(EBDEV) task test_smoke

smoke-tui:
	$(EBDEV) task test_smoke --tui

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
	$(EBDEV) task test_complex --tui --debug-log /tmp/ebdev-debug.json
	@echo ""
	@echo "Debug log written to /tmp/ebdev-debug.json"

smoke-debug:
	$(EBDEV) task test_smoke --tui --debug-log /tmp/ebdev-debug.json
	@echo ""
	@echo "Debug log written to /tmp/ebdev-debug.json"

# Build
build:
	cd .. && cargo build -p ebdev
	$(EBDEV) task hello
