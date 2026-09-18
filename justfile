# Storage Monitor task runner. Install with: brew install just

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

# Production build of the frontend (what `tauri build` runs first)
build-web:
    pnpm --filter @storage-monitor/desktop build

# Release build of the app bundle for this machine
build:
    pnpm --filter @storage-monitor/desktop tauri build

# Clippy with the stable toolchain CI uses (needs Docker); core and cli only
clippy-ci:
    docker run --rm -v "$PWD":/w -w /w -e CARGO_TARGET_DIR=/tmp/target -e CARGO_HOME=/tmp/cargo rust:latest cargo clippy --workspace --exclude storage-monitor-desktop --all-targets -- -D warnings

# Everything CI runs, except the macOS `tauri build` smoke
ci: lint test build-web e2e
