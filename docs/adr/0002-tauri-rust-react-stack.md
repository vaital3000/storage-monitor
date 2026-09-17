# 2. Tauri 2 with a Rust core and a React UI

Date: 2026-09-17. Status: accepted.

## Context

Options considered: native SwiftUI, Tauri 2 (Rust + web UI), and a CLI with a
local web dashboard. The product needs a fast parallel scanner, a rich
dashboard (treemap, tables), a downloadable app from CI, and a test suite that
runs on cheap Linux runners and that an agent can verify in a browser.

## Decision

Tauri 2. All logic lives in Rust crates that know nothing about Tauri; the
desktop app and the CLI are adapters. The UI is React + TypeScript with
Tailwind. The frontend runs standalone with mocked IPC for tests and review.

## Consequences

Native look is approximated, not exact. Two toolchains (cargo, pnpm). In
exchange: Rust and frontend tests run on Linux, the same core powers a CLI,
and UI work is reviewable in any browser.
