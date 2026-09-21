CARGO ?= cargo
FILE  ?=

.PHONY: help build check test run repl fmt fmt-check lint ci clean

build:
	$(CARGO) build

check:
	$(CARGO) check --all-targets

test:
	$(CARGO) test

run:
	@$(CARGO) run -q -p ludo -- $(FILE)

fmt:
	$(CARGO) fmt --all

clippy:
	$(CARGO) clippy --all-targets -- -D warnings

lint: fmt clippy

clean:
	$(CARGO) clean
