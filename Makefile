.PHONY: build test lint format check setup \
       promote promote-alpha promote-beta promote-rc promote-release

check: lint test

build:
	cargo build --release

test:
	cargo test --locked

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings

format:
	cargo fmt --all

setup:
	cargo fetch

promote:
	@scripts/promote-release.sh

promote-alpha promote-beta promote-rc promote-release:
	@scripts/promote-release.sh $(subst promote-,,$@)
