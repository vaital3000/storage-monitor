# Storage Monitor task runner. Install with: brew install just

set shell := ["zsh", "-cu"]

default:
    @just --list

# Install JS dependencies and the Playwright browser
setup:
    pnpm install
    pnpm --filter @storage-monitor/desktop exec playwright install chromium

# Run the desktop app in development mode
dev:
    pnpm --filter @storage-monitor/desktop tauri dev

# Run the frontend alone in a browser with mocked IPC (http://localhost:1420)
dev-web:
    pnpm --filter @storage-monitor/desktop exec vite --mode mock

# All unit tests
test: test-rust test-web

test-rust:
    cargo test --workspace

test-web:
    pnpm --filter @storage-monitor/desktop test

# End-to-end tests against the mocked frontend
e2e:
    pnpm --filter @storage-monitor/desktop e2e

# Same checks as CI
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    pnpm --filter @storage-monitor/desktop typecheck
    pnpm --filter @storage-monitor/desktop lint
    pnpm format:check

# Apply formatters
fmt:
    cargo fmt --all
    pnpm format

# Release build of the app bundle for this machine
build:
    pnpm --filter @storage-monitor/desktop tauri build

# Everything CI runs
ci: lint test e2e
