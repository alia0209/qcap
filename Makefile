.PHONY: validate fmt lint test build cli install

validate: fmt lint test build cli

fmt:
	cargo fmt --check

lint:
	cargo clippy --all-targets -- -D warnings
	shellcheck test/*.sh bench/*.sh

test:
	cargo test --quiet

build:
	cargo build --release --quiet

cli: build
	bash test/cli.test.sh

install: build
	mkdir -p $(HOME)/.local/bin
	ln -sfn $(CURDIR)/target/release/q $(HOME)/.local/bin/q
