CARGO ?= cargo
FILE  ?=

.PHONY: build check test run play install fmt clippy lint bench clean

build:
	$(CARGO) build

check:
	$(CARGO) check --all-targets

test:
	$(CARGO) test

run:
	@$(CARGO) run -q -p ludo -- run $(FILE)

play:
	@$(CARGO) run -q -p ludo -- play $(FILE)

install:
	$(CARGO) install --path crates/cli

fmt:
	$(CARGO) fmt --all

clippy:
	$(CARGO) clippy --all-targets -- -D warnings

lint: fmt clippy

bench:
	./bench/run.sh

clean:
	$(CARGO) clean
