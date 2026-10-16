SHELL := /bin/bash
.SHELLFLAGS := -euo pipefail -c
.DEFAULT_GOAL := help
# Per-checkout targets avoid Cargo lock contention; sccache shares compiled dependencies.
export CARGO_TARGET_DIR ?= $(CURDIR)/target
SCCACHE := $(shell command -v sccache 2>/dev/null)
ifneq ($(SCCACHE),)
export RUSTC_WRAPPER ?= $(SCCACHE)
endif
.PHONY: help setup fmt fmt-check lint test test-unit test-e2e test-doc test-native web-build types ci build dev package server clean cache-info
help: ## Show developer commands
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-16s %s\n", $$1, $$2}' $(MAKEFILE_LIST)
setup: ## Install frontend dependencies, respecting lockfile
	npm ci
.PHONY: icons
icons: ## Regenerate desktop icons from the vector source
	bash scripts/icons.sh
types: ## Generate UI types from Rust wire types
	cargo run -q -p latte-work-protocol --bin export-types --locked > apps/desktop/src/protocol.ts
fmt: ## Format Rust and frontend
	cargo fmt --all
	npm run format
fmt-check: ## Check Rust and frontend formatting
	cargo fmt --all -- --check
	npm run format:check
lint: prepare ## Check all Rust crates, denying warnings
	cargo clippy --workspace --all-targets --locked -- -D warnings
test-unit: ## Rust crate-local unit tests and frontend reducers
	cargo test --workspace --exclude latte-work-desktop --lib --bins --locked
	npm test
test-e2e: ## Final server binary, socket bridge, and deterministic Claude fixture
	cargo test -p latte-work-server --test e2e --locked -- --test-threads=1
test-doc: ## Rust documentation tests
	cargo test --workspace --exclude latte-work-desktop --doc --locked
test-native: ## macOS WebKit site-data boundaries and localhost favicon transport
	python3 scripts/test-site-data.py
	python3 scripts/test-site-icons.py
test: test-unit test-e2e test-doc ## Run every supported test layer (including native on macOS)
ifeq ($(shell uname -s),Darwin)
test: test-native
endif
web-build: ## Typecheck and bundle React
	npm run web-build
ci: fmt-check types-check lint-ci lint test web-build ## Full local gate
	RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
build: ## Build server and native app
	./scripts/desktop.sh build
dev: ## Launch real desktop with development UI
	./scripts/desktop.sh dev
package: ## Build release app/server and verified drag-install DMG on macOS
	./scripts/desktop.sh package
server: ## Start host service in foreground
	cargo run -p latte-work-server --locked -- serve
clean: ## Remove only this checkout's Cargo build artifacts
	cargo clean
cache-info: ## Show build cache configuration
	@echo "CARGO_TARGET_DIR=$(CARGO_TARGET_DIR)"
	@echo "RUSTC_WRAPPER=$${RUSTC_WRAPPER:-disabled (install sccache to enable)}"
	@if command -v sccache >/dev/null; then sccache --show-stats; fi

.PHONY: prepare
prepare: ## Prepare native sidecar before checking desktop
	./scripts/desktop.sh prepare

.PHONY: types-check
types-check: ## Check generated Rust/TypeScript protocol stays synchronized
	./scripts/check-types.sh

.PHONY: lint-ci
lint-ci: ## Validate GitHub workflow and shell scripts
	actionlint
	shellcheck scripts/*.sh

.PHONY: package-dmg
package-dmg: ## Repackage the existing signed release app without rebuilding
	./scripts/package-dmg.sh
