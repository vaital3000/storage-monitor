# Phase 0: Walking Skeleton Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Ship `v0.1.0` of Storage Monitor: an empty-but-real Tauri app plus a CLI, built and attached to a GitHub Release by CI, in a repository an agent can operate without help.

**Architecture:** Cargo workspace (`crates/core`, `crates/cli`, `apps/desktop/src-tauri`) plus a pnpm workspace (`apps/desktop`). The core crate owns all logic; the CLI and the Tauri app are thin adapters. One end-to-end slice exists: core `app_info()` → Tauri command `get_app_info` → React shows the version; the same function backs `storage-monitor info --json`. The frontend can run in a browser with mocked IPC, which is how Playwright and the maintainer's browser verify it.

**Tech Stack:** Rust 2024 edition, Tauri 2.x, React 19, TypeScript, Vite, Tailwind v4, Vitest + Testing Library, Playwright, pnpm 10, `just`, GitHub Actions, release-please (Rust strategy), conventional commits.

**Design reference:** `docs/plans/2026-09-17-storage-monitor-design.md` (sections 5, 12, 13, 14).

**Conventions for every task:**
- Work on branch `feat/phase-0-walking-skeleton` in a dedicated worktree. Never commit to `main`.
- Commit after each task with a conventional commit message. Small commits, one concern each.
- Every command below is run from the repository root unless stated otherwise.
- If a generated file differs from what this plan shows, the plan wins: overwrite it.
- Versions: install with `@latest` / caret ranges and let lockfiles pin. Exception: if `tsc` fails because the scaffold installed TypeScript 7, pin `typescript@~5.9` and continue.

---

### Task 0: Prerequisites and worktree

**Step 1: Install missing local tools**

Run:
```bash
brew install just
```
Expected: `just --version` prints a version. (Rust is installed via Homebrew without rustup, so `rust-toolchain.toml` is ignored locally; that is fine. CI uses rustup.)

**Step 2: Create the worktree and branch**

Use the superpowers:using-git-worktrees skill. Branch name: `feat/phase-0-walking-skeleton`, based on `main`.

**Step 3: Verify the starting point**

Run: `git log --oneline -3`
Expected: the top commit is `docs: add v1 product and architecture design (#1)`.

---

### Task 1: Repository hygiene files

**Files:**
- Modify: `.gitignore` (replace the Node template completely)
- Create: `.editorconfig`, `rust-toolchain.toml`, `rustfmt.toml`, `.prettierrc`, `.prettierignore`

**Step 1: Write `.gitignore`**

```gitignore
# macOS
.DS_Store

# Git worktrees created by tooling
.worktrees/

# Rust
/target/
**/*.rs.bk

# Node
node_modules/
*.log
.pnpm-debug.log*

# Frontend build output
apps/desktop/dist/

# Tauri generated files
apps/desktop/src-tauri/gen/
apps/desktop/src-tauri/target/

# Test artifacts
apps/desktop/coverage/
apps/desktop/playwright-report/
apps/desktop/test-results/
apps/desktop/blob-report/

# Editors
.idea/
.vscode/*
!.vscode/extensions.json
*.swp

# Local environment files (.env.mock is committed on purpose)
.env
.env.local
.env.*.local
```

**Step 2: Write `.editorconfig`**

```ini
root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
indent_style = space
indent_size = 2

[*.rs]
indent_size = 4

[*.md]
trim_trailing_whitespace = false

[justfile]
indent_size = 4
```

**Step 3: Write `rust-toolchain.toml`**

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

**Step 4: Write `rustfmt.toml`**

```toml
edition = "2024"
```

**Step 5: Write `.prettierrc`**

```json
{
  "singleQuote": true,
  "semi": true,
  "printWidth": 100,
  "trailingComma": "all"
}
```

**Step 6: Write `.prettierignore`**

```
node_modules
dist
target
pnpm-lock.yaml
CHANGELOG.md
apps/desktop/src-tauri/gen
apps/desktop/src-tauri/icons
apps/desktop/playwright-report
apps/desktop/test-results
```

**Step 7: Commit**

```bash
git add .gitignore .editorconfig rust-toolchain.toml rustfmt.toml .prettierrc .prettierignore
git commit -m "chore: add editor, formatter and toolchain configuration"
```

---

### Task 2: Cargo workspace and the core crate

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/core/Cargo.toml`
- Create: `crates/core/src/lib.rs`

Important: `members` must be listed explicitly (no globs). release-please's Rust strategy reads this list to bump every crate's version, and each crate must carry its own literal `version = "0.0.0"` (not `version.workspace = true`).

**Step 1: Write the workspace `Cargo.toml`**

```toml
[workspace]
resolver = "3"
members = ["crates/core"]

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT"
repository = "https://github.com/vaital3000/storage-monitor"
authors = ["Vitaliy Pomozov"]

[workspace.dependencies]
storage-monitor-core = { path = "crates/core" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }

[profile.release]
codegen-units = 1
lto = true
strip = true
```

Members are added as the crates appear: Task 3 adds `"crates/cli"`, Task 4 adds `"apps/desktop/src-tauri"`.

**Step 2: Write `crates/core/Cargo.toml`**

```toml
[package]
name = "storage-monitor-core"
version = "0.0.0"
description = "Core library of Storage Monitor: disk scanner, module contract, action engine"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
```

**Step 3: Write the failing tests in `crates/core/src/lib.rs`**

```rust
//! Core library of Storage Monitor.
//!
//! Phase 0 exposes only application metadata. The scanner, the module
//! contract and the action engine arrive in later phases; see
//! `docs/plans/2026-09-17-storage-monitor-design.md`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_info_reports_name_and_crate_version() {
        let info = app_info();
        assert_eq!(info.name, "Storage Monitor");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn app_info_serializes_to_json() {
        let json = serde_json::to_value(app_info()).unwrap();
        assert_eq!(json["name"], "Storage Monitor");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    }
}
```

**Step 4: Run the tests to verify they fail**

Run: `cargo test -p storage-monitor-core`
Expected: compile error, `cannot find function app_info in this scope`.

**Step 5: Implement**

Insert above the `#[cfg(test)]` block in `crates/core/src/lib.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Human-readable product name.
pub const APP_NAME: &str = "Storage Monitor";

/// Application metadata shared by the desktop app and the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
}

/// Returns the product name and the crate version compiled into the binary.
pub fn app_info() -> AppInfo {
    AppInfo {
        name: APP_NAME.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}
```

**Step 6: Run the tests to verify they pass**

Run: `cargo test -p storage-monitor-core`
Expected: `test result: ok. 2 passed`.

**Step 7: Lint**

Run: `cargo fmt --all --check && cargo clippy -p storage-monitor-core --all-targets -- -D warnings`
Expected: no output, exit code 0.

**Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock crates/core
git commit -m "feat(core): add workspace and app metadata"
```

---

### Task 3: CLI crate

**Files:**
- Modify: `Cargo.toml` (add `"crates/cli"` to `members`)
- Create: `crates/cli/Cargo.toml`
- Create: `crates/cli/src/main.rs`
- Create: `crates/cli/tests/cli.rs`

**Step 1: Register the member and write `crates/cli/Cargo.toml`**

In the root `Cargo.toml` set `members = ["crates/core", "crates/cli"]`. Then write:

```toml
[package]
name = "storage-monitor-cli"
version = "0.0.0"
description = "Command-line interface of Storage Monitor"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[[bin]]
name = "storage-monitor"
path = "src/main.rs"

[dependencies]
storage-monitor-core = { workspace = true }
clap = { workspace = true }
serde_json = { workspace = true }
```

**Step 2: Write the failing integration test `crates/cli/tests/cli.rs`**

```rust
use std::process::Command;

fn storage_monitor() -> Command {
    Command::new(env!("CARGO_BIN_EXE_storage-monitor"))
}

#[test]
fn info_json_prints_name_and_version() {
    let output = storage_monitor()
        .args(["info", "--json"])
        .output()
        .expect("binary runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(json["name"], "Storage Monitor");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn info_plain_prints_one_line() {
    let output = storage_monitor().arg("info").output().expect("binary runs");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        text.trim(),
        format!("Storage Monitor {}", env!("CARGO_PKG_VERSION"))
    );
}
```

**Step 3: Write a placeholder `crates/cli/src/main.rs` so the test compiles, then run it to see it fail**

```rust
fn main() {}
```

Run: `cargo test -p storage-monitor-cli`
Expected: both tests FAIL (`info_json_prints_name_and_version` panics on `valid JSON`, the plain test on the assertion).

**Step 4: Implement `crates/cli/src/main.rs`**

```rust
use clap::{Parser, Subcommand};
use storage_monitor_core::app_info;

#[derive(Parser)]
#[command(
    name = "storage-monitor",
    version,
    about = "Disk space analyzer with cleanup modules"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print application metadata
    Info {
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Info { json } => {
            let info = app_info();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&info).expect("AppInfo is serializable")
                );
            } else {
                println!("{} {}", info.name, info.version);
            }
        }
    }
}
```

**Step 5: Run the tests to verify they pass**

Run: `cargo test -p storage-monitor-cli`
Expected: `test result: ok. 2 passed`.

**Step 6: Lint and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: exit code 0.

```bash
git add Cargo.toml Cargo.lock crates/cli
git commit -m "feat(cli): add storage-monitor binary with info command"
```

---

### Task 4: Desktop app scaffold and the first IPC command

**Files:**
- Create (scaffold): `apps/desktop/**` via `create-tauri-app`
- Create: `pnpm-workspace.yaml`, `package.json` (root)
- Modify: `Cargo.toml` (add the third workspace member)
- Overwrite: `apps/desktop/package.json`, `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/src-tauri/capabilities/default.json`, `apps/desktop/src-tauri/src/lib.rs`, `apps/desktop/src-tauri/src/main.rs`
- Delete: template leftovers listed in Step 5

**Step 1: Scaffold**

Run:
```bash
mkdir -p apps && cd apps && pnpm create tauri-app@latest --help
```
Read the flags, then run the non-interactive scaffold (adjust flag names if the help output differs):
```bash
pnpm create tauri-app@latest desktop --template react-ts --manager pnpm --yes
cd ..
```
Expected: `apps/desktop` exists with `src/`, `src-tauri/`, `package.json`, `vite.config.ts`, `tsconfig.json`, `index.html`. Do not run `pnpm install` inside `apps/desktop`; the workspace install happens in Step 4.

**Step 2: Write `pnpm-workspace.yaml`**

```yaml
packages:
  - 'apps/*'
```

**Step 3: Write the root `package.json`**

```json
{
  "name": "storage-monitor",
  "version": "0.0.0",
  "private": true,
  "packageManager": "pnpm@10.28.1",
  "engines": {
    "node": ">=22"
  },
  "scripts": {
    "dev": "pnpm --filter @storage-monitor/desktop tauri dev",
    "build": "pnpm --filter @storage-monitor/desktop tauri build",
    "format": "prettier --write .",
    "format:check": "prettier --check ."
  },
  "devDependencies": {
    "prettier": "^3.9.0"
  }
}
```

**Step 4: Overwrite `apps/desktop/package.json`**

Keep the dependency versions the scaffold chose for anything already present; add the rest with `pnpm add` in Step 6. The file must end up with these names and scripts:

```json
{
  "name": "@storage-monitor/desktop",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc --noEmit && vite build",
    "preview": "vite preview",
    "tauri": "tauri",
    "typecheck": "tsc --noEmit",
    "lint": "eslint .",
    "test": "vitest run",
    "test:watch": "vitest",
    "e2e": "playwright test"
  },
  "dependencies": {
    "@tauri-apps/api": "^2",
    "react": "^19",
    "react-dom": "^19"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2",
    "@types/react": "^19",
    "@types/react-dom": "^19",
    "@vitejs/plugin-react": "^6",
    "typescript": "~5.9",
    "vite": "^8"
  }
}
```

Then run from the root:
```bash
pnpm install
```
Expected: lockfile `pnpm-lock.yaml` created at the root; `apps/desktop/node_modules` present. If pnpm prints "Ignored build scripts", run `pnpm approve-builds`, select everything listed, and commit the resulting `pnpm-workspace.yaml` change.

**Step 5: Delete template leftovers**

```bash
rm -f apps/desktop/src/App.css apps/desktop/src/assets/react.svg apps/desktop/public/vite.svg apps/desktop/public/tauri.svg apps/desktop/pnpm-lock.yaml
rm -rf apps/desktop/.git apps/desktop/.gitignore apps/desktop/README.md
```
Keep `apps/desktop/src-tauri/icons/` (default Tauri icons; a custom icon is a phase 6 task) and `apps/desktop/src-tauri/build.rs`.

**Step 6: Add the workspace member**

In the root `Cargo.toml` set `members = ["crates/core", "crates/cli", "apps/desktop/src-tauri"]`.

**Step 7: Overwrite `apps/desktop/src-tauri/Cargo.toml`**

```toml
[package]
name = "storage-monitor-desktop"
version = "0.0.0"
description = "Storage Monitor desktop app"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[lib]
name = "storage_monitor_desktop_lib"
crate-type = ["rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
storage-monitor-core = { workspace = true }
tauri = { version = "2", features = [] }
```

**Step 8: Overwrite `apps/desktop/src-tauri/tauri.conf.json`**

`version` is intentionally absent: Tauri reads it from `src-tauri/Cargo.toml`, which release-please bumps.

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Storage Monitor",
  "identifier": "io.github.vaital3000.storage-monitor",
  "build": {
    "beforeDevCommand": "pnpm dev",
    "devUrl": "http://localhost:1420",
    "beforeBuildCommand": "pnpm build",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [
      {
        "title": "Storage Monitor",
        "width": 1200,
        "height": 800,
        "minWidth": 900,
        "minHeight": 600,
        "titleBarStyle": "Overlay",
        "hiddenTitle": true
      }
    ],
    "security": {
      "csp": null
    }
  },
  "bundle": {
    "active": true,
    "targets": ["app", "dmg"],
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ],
    "macOS": {
      "minimumSystemVersion": "13.3",
      "signingIdentity": "-"
    }
  }
}
```

**Step 9: Overwrite `apps/desktop/src-tauri/capabilities/default.json`**

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Capability for the main window",
  "windows": ["main"],
  "permissions": ["core:default"]
}
```

**Step 10: Write the failing Rust test in `apps/desktop/src-tauri/src/lib.rs`**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn get_app_info_returns_core_metadata() {
        assert_eq!(super::get_app_info(), storage_monitor_core::app_info());
    }
}
```

Run: `cargo test -p storage-monitor-desktop`
Expected: compile error, `get_app_info` not found.

**Step 11: Implement `apps/desktop/src-tauri/src/lib.rs`**

Insert above the test module:

```rust
use storage_monitor_core::{AppInfo, app_info};

/// Returns product name and version to the UI.
#[tauri::command]
fn get_app_info() -> AppInfo {
    app_info()
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_app_info])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

And `apps/desktop/src-tauri/src/main.rs`:

```rust
// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    storage_monitor_desktop_lib::run()
}
```

**Step 12: Run the tests to verify they pass**

Run: `cargo test -p storage-monitor-desktop`
Expected: `test result: ok. 1 passed`. The first build compiles Tauri and takes a few minutes.

**Step 13: Lint and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: exit 0.

```bash
git add -A
git commit -m "feat(desktop): scaffold Tauri app with get_app_info command"
```

---

### Task 5: Frontend: Tailwind, typed IPC, mock mode, unit test

**Files:**
- Overwrite: `apps/desktop/vite.config.ts`, `apps/desktop/src/main.tsx`, `apps/desktop/src/App.tsx`, `apps/desktop/src/index.css`, `apps/desktop/index.html`
- Create: `apps/desktop/src/lib/ipc.ts`, `apps/desktop/src/mocks/ipc.ts`, `apps/desktop/.env.mock`, `apps/desktop/vitest.config.ts`, `apps/desktop/src/test/setup.ts`, `apps/desktop/src/App.test.tsx`

**Step 1: Install dependencies**

```bash
pnpm --filter @storage-monitor/desktop add tailwindcss @tailwindcss/vite
pnpm --filter @storage-monitor/desktop add -D vitest jsdom @testing-library/react @testing-library/jest-dom @testing-library/dom
```
Expected: no peer-dependency errors. If `@tailwindcss/vite` refuses the installed Vite major, use the PostCSS route instead (`@tailwindcss/postcss` in `postcss.config.js`) and drop the Vite plugin line below.

**Step 2: Overwrite `apps/desktop/vite.config.ts`**

```ts
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Tauri prints its own errors; keep them visible.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  envPrefix: ['VITE_'],
  build: {
    // macOS 13.3+ = Safari 16.4, required by Tailwind v4.
    target: 'safari16',
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
```

**Step 3: Write `apps/desktop/src/lib/ipc.ts`**

Every backend call goes through this file so the mock in `src/mocks/ipc.ts` and the real commands stay in one place.

```ts
import { invoke } from '@tauri-apps/api/core';

export interface AppInfo {
  name: string;
  version: string;
}

export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>('get_app_info');
}
```

**Step 4: Write `apps/desktop/src/mocks/ipc.ts`**

```ts
import { mockIPC } from '@tauri-apps/api/mocks';
import type { AppInfo } from '../lib/ipc';

export const MOCK_APP_INFO: AppInfo = { name: 'Storage Monitor', version: '0.0.0-mock' };

/** Installs fake handlers for every backend command. Used by unit tests, e2e and browser dev. */
export function installIpcMock(): void {
  mockIPC((cmd) => {
    switch (cmd) {
      case 'get_app_info':
        return MOCK_APP_INFO;
      default:
        throw new Error(`Unmocked IPC command: ${cmd}`);
    }
  });
}
```

**Step 5: Write `apps/desktop/.env.mock`**

```
VITE_MOCK_IPC=1
```

**Step 6: Overwrite `apps/desktop/src/main.tsx`**

```tsx
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { installIpcMock } from './mocks/ipc';
import './index.css';

// `vite --mode mock` loads .env.mock and lets the UI run in a plain browser.
if (import.meta.env.VITE_MOCK_IPC === '1') {
  installIpcMock();
}

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
```

**Step 7: Overwrite `apps/desktop/src/index.css`**

```css
@import 'tailwindcss';

:root {
  color-scheme: light dark;
  font-family:
    -apple-system, BlinkMacSystemFont, 'SF Pro Text', system-ui, sans-serif;
  -webkit-font-smoothing: antialiased;
}

html,
body,
#root {
  height: 100%;
  margin: 0;
}
```

**Step 8: Overwrite `apps/desktop/index.html`**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Storage Monitor</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

**Step 9: Write the vitest config and setup**

`apps/desktop/vitest.config.ts`:

```ts
import { defineConfig, mergeConfig } from 'vitest/config';
import viteConfig from './vite.config.ts';

export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      environment: 'jsdom',
      setupFiles: ['./src/test/setup.ts'],
      include: ['src/**/*.test.{ts,tsx}'],
    },
  }),
);
```

`apps/desktop/src/test/setup.ts`:

```ts
import '@testing-library/jest-dom/vitest';
import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';
import { clearMocks } from '@tauri-apps/api/mocks';

// jsdom 30+ ships WebCrypto, which @tauri-apps/api needs for callback ids.
afterEach(() => {
  cleanup();
  clearMocks();
});
```

**Step 10: Write the failing unit test `apps/desktop/src/App.test.tsx`**

```tsx
import { mockIPC } from '@tauri-apps/api/mocks';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import App from './App';
import { installIpcMock } from './mocks/ipc';

describe('App', () => {
  it('shows the product name and the version reported by the backend', async () => {
    installIpcMock();
    render(<App />);
    expect(await screen.findByRole('heading', { name: 'Storage Monitor' })).toBeInTheDocument();
    expect(await screen.findByText('v0.0.0-mock')).toBeInTheDocument();
  });

  it('shows the backend error when the command fails', async () => {
    mockIPC(() => {
      throw new Error('boom');
    });
    render(<App />);
    expect(await screen.findByText('Error: boom')).toBeInTheDocument();
  });
});
```

Run: `pnpm --filter @storage-monitor/desktop test`
Expected: FAIL (the scaffolded `App.tsx` renders the template greeting, no `v0.0.0-mock`).

**Step 11: Overwrite `apps/desktop/src/App.tsx`**

```tsx
import { useEffect, useState } from 'react';
import { getAppInfo, type AppInfo } from './lib/ipc';

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getAppInfo()
      .then(setInfo)
      .catch((e: unknown) => setError(String(e)));
  }, []);

  return (
    <main className="flex h-full flex-col items-center justify-center gap-2 bg-neutral-50 text-neutral-900 dark:bg-neutral-950 dark:text-neutral-50">
      <h1 className="text-2xl font-semibold">{info?.name ?? 'Storage Monitor'}</h1>
      <p className="text-sm text-neutral-500" data-testid="version">
        {error ?? (info ? `v${info.version}` : 'Loading…')}
      </p>
    </main>
  );
}
```

**Step 12: Run the unit test to verify it passes**

Run: `pnpm --filter @storage-monitor/desktop test`
Expected: `2 passed`.

**Step 13: Typecheck and run the real app once**

Run: `pnpm --filter @storage-monitor/desktop typecheck`
Expected: exit 0.

Run: `pnpm dev` (from the root), wait for the window, confirm it shows "Storage Monitor" and "v0.0.0", then stop it with Ctrl+C.
Optional visual proof: `screencapture -x /tmp/phase0.png` while the window is frontmost.

**Step 14: Commit**

```bash
git add -A
git commit -m "feat(desktop): typed IPC layer, mock mode and first screen"
```

---

### Task 6: Playwright e2e against the mocked frontend

**Files:**
- Create: `apps/desktop/playwright.config.ts`, `apps/desktop/e2e/smoke.spec.ts`

**Step 1: Install**

```bash
pnpm --filter @storage-monitor/desktop add -D @playwright/test
pnpm --filter @storage-monitor/desktop exec playwright install chromium
```

**Step 2: Write `apps/desktop/playwright.config.ts`**

The dev server runs on port 1430 so it never collides with `tauri dev` on 1420.

```ts
import { defineConfig, devices } from '@playwright/test';

const PORT = 1430;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? [['github'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: `pnpm exec vite --mode mock --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
```

**Step 3: Write the failing e2e test `apps/desktop/e2e/smoke.spec.ts`**

```ts
import { expect, test } from '@playwright/test';

test('renders the app name and the mocked backend version', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Storage Monitor' })).toBeVisible();
  await expect(page.getByTestId('version')).toHaveText('v0.0.0-mock');
});

test('takes a screenshot for the PR', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('version')).toHaveText('v0.0.0-mock');
  await page.screenshot({ path: test.info().outputPath('home.png'), fullPage: true });
});
```

**Step 4: Run it**

Run: `pnpm --filter @storage-monitor/desktop e2e`
Expected: `2 passed`. (It passes immediately because Task 5 already built the screen; the value of this task is the harness. If it fails, fix the harness, not the test.)

**Step 5: Commit**

```bash
git add apps/desktop/playwright.config.ts apps/desktop/e2e pnpm-lock.yaml apps/desktop/package.json
git commit -m "test(desktop): add Playwright smoke test in mock mode"
```

---

### Task 7: ESLint, Prettier and the justfile

**Files:**
- Create: `apps/desktop/eslint.config.js`, `justfile`

**Step 1: Install ESLint**

```bash
pnpm --filter @storage-monitor/desktop add -D eslint @eslint/js globals typescript-eslint eslint-plugin-react-hooks eslint-plugin-react-refresh eslint-config-prettier
```

**Step 2: Write `apps/desktop/eslint.config.js`**

```js
import js from '@eslint/js';
import globals from 'globals';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import tseslint from 'typescript-eslint';
import prettier from 'eslint-config-prettier';
import { defineConfig } from 'eslint/config';

export default defineConfig(
  { ignores: ['dist', 'src-tauri', 'playwright-report', 'test-results'] },
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      ...tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
      prettier,
    ],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
  },
);
```

If `reactHooks.configs.flat` is undefined for the installed plugin version, use `reactHooks.configs['recommended-latest']` instead. If the scaffold already produced an `eslint.config.js`, replace it with this one.

**Step 3: Run lint and format**

```bash
pnpm --filter @storage-monitor/desktop lint
pnpm format
pnpm format:check
```
Expected: lint exit 0; `format` rewrites files; `format:check` exit 0. Commit the reformatted files.

**Step 4: Write `justfile`**

```make
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

# Everything CI runs, except the macOS `tauri build` smoke
ci: lint test build-web e2e
```

**Step 5: Run the whole gate**

Run: `just ci`
Expected: every step passes.

**Step 6: Commit**

```bash
git add -A
git commit -m "chore: add eslint, prettier formatting and just recipes"
```

---

### Task 8: Repository documentation for humans and agents

**Files:**
- Overwrite: `README.md`
- Create: `CLAUDE.md`, `AGENTS.md` (symlink), `CONTRIBUTING.md`, `SECURITY.md`, `.github/CODEOWNERS`, `.github/PULL_REQUEST_TEMPLATE.md`, `.github/ISSUE_TEMPLATE/bug_report.yml`, `.github/ISSUE_TEMPLATE/module_request.yml`, `.github/ISSUE_TEMPLATE/config.yml`, `docs/adr/0001-record-architecture-decisions.md`, `docs/adr/0002-tauri-rust-react-stack.md`, `docs/adr/0003-trash-first-dual-deletion.md`

**Step 1: Write `README.md`**

```markdown
# Storage Monitor

[![CI](https://github.com/vaital3000/storage-monitor/actions/workflows/ci.yml/badge.svg)](https://github.com/vaital3000/storage-monitor/actions/workflows/ci.yml)

A macOS disk space analyzer that knows what your developer artifacts are and
can clean them up safely: git worktrees, Docker images and caches, Xcode
DerivedData and simulators.

Most analyzers answer "where is the space". Storage Monitor answers "what can I
free right now, and why", with a preview and a confirmation before anything is
touched. Deletion goes to the Trash by default.

> Status: pre-alpha. Phase 0 (walking skeleton) is done; the scanner and the
> modules are being built. See the [roadmap](docs/plans/2026-09-17-storage-monitor-design.md#14-roadmap).

## Install

Download the latest `.dmg` from [Releases](https://github.com/vaital3000/storage-monitor/releases),
open it and drag the app to Applications.

The app is not notarized yet. On first launch macOS will refuse to open it:
right-click the app, choose **Open**, and confirm. Alternatively:

```bash
xattr -d com.apple.quarantine "/Applications/Storage Monitor.app"
```

The same release contains `storage-monitor-cli-<version>-macos-universal.tar.gz`
with the command-line tool.

## Development

Prerequisites: Rust (stable), Node 22+, pnpm 10, [`just`](https://github.com/casey/just).

```bash
just setup   # install JS deps and the Playwright browser
just dev     # run the desktop app
just dev-web # run only the UI in a browser with mocked backend
just ci      # lint + unit tests + e2e, same as CI
```

Layout, conventions and the module contract are described in
[CLAUDE.md](CLAUDE.md) and [docs/](docs/). Design decisions live in
[docs/adr](docs/adr).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Issues and pull requests are welcome.

## License

[MIT](LICENSE)
```

**Step 2: Write `CLAUDE.md`**

```markdown
# Storage Monitor: guide for coding agents

Read this before changing anything. `AGENTS.md` is a symlink to this file.

## What this is

A macOS disk space analyzer (Tauri 2, Rust core, React UI) with cleanup
modules for developer artifacts. The full design is in
`docs/plans/2026-09-17-storage-monitor-design.md`; decisions are in
`docs/adr/`. Implementation plans live in `docs/plans/`.

## Layout

```
crates/core/              Rust library: all logic lives here (scanner, modules, actions)
crates/modules/<id>/      One crate per cleanup module (from phase 3 on)
crates/cli/               `storage-monitor` binary, thin wrapper over core, JSON output
apps/desktop/src-tauri/   Tauri commands and app state, thin wrapper over core
apps/desktop/src/         React UI. Backend calls only through src/lib/ipc.ts
apps/desktop/src/mocks/   IPC mocks used by unit tests, e2e and `just dev-web`
docs/adr/                 Architecture decision records
docs/plans/               Designs and implementation plans
```

Dependency direction: `core` ← `modules` ← `cli` / `desktop`. Core never
imports Tauri. The UI never calls `invoke` outside `src/lib/ipc.ts`.

## Commands

```
just setup      install JS deps and Playwright browser
just dev        run the desktop app
just dev-web    run the UI in a browser with mocked IPC (fastest UI loop)
just test       cargo test + vitest
just e2e        Playwright against the mocked UI
just lint       fmt, clippy -D warnings, tsc, eslint, prettier
just fmt        apply formatters
just ci         lint + test + e2e, identical to CI
```

Rust tests for a single crate: `cargo test -p storage-monitor-core`.
One vitest file: `pnpm --filter @storage-monitor/desktop test src/App.test.tsx`.

## Workflow

- Never commit to `main`. Branch, open a PR, wait for green CI, squash-merge.
- Conventional commits and PR titles: `feat(core): ...`, `fix(desktop): ...`,
  `docs: ...`, `chore: ...`, `ci: ...`, `test: ...`, `refactor: ...`.
  Releases and the changelog are generated from them by release-please.
- TDD: write the failing test first, then the minimal implementation.
- Every module ships with fixtures for external commands (`gh`, `docker`,
  `xcrun`) replayed through the fake `System`; tests must pass on Linux.
- UI changes: add or update a Playwright test and attach its screenshot to the PR.

## Definition of Done

- `just ci` passes locally and in CI.
- New behavior has tests; verdict rules and action safety have unit tests.
- Docs updated: the module guide for new modules, ADR for a changed decision,
  `CLAUDE.md` for new commands or layout changes.
- No shell strings: external commands are argv arrays through `System`.
- Deletion code paths respect the allowed roots and the Trash-by-default rule.

## Conventions

- Rust 2024 edition, `clippy -D warnings`, `rustfmt` defaults.
- TypeScript strict, ESLint + Prettier, Tailwind for styling, no CSS modules.
- English for code, docs, UI strings and commit messages.
- Do not add dependencies for something the standard library or an existing
  dependency already does.

## Release

Merging to `main` lets release-please maintain a release PR. Merging that PR
tags `vX.Y.Z`, creates the GitHub Release, and `release.yml` attaches the
universal `.dmg` and the CLI tarball. Versions live in each crate's
`Cargo.toml` and in `package.json`; never edit them by hand.
```

Then create the symlink:
```bash
ln -s CLAUDE.md AGENTS.md
```

**Step 3: Write `CONTRIBUTING.md`**

```markdown
# Contributing

Thanks for helping. This project is developed with AI coding agents in the
loop, so the repository is documented for them as much as for humans; start
with [CLAUDE.md](CLAUDE.md).

## Setup

Rust stable, Node 22+, pnpm 10 and `just`. Then `just setup` and `just ci`.

## Workflow

1. Open an issue or pick one. Module ideas use the "Module request" template.
2. Branch from `main`, keep the change focused.
3. Follow TDD; `just ci` must pass.
4. Open a PR with a conventional title (`feat(core): ...`). Fill the template.
5. A maintainer squash-merges after CI is green.

## Commit messages

Conventional commits. Types: `feat`, `fix`, `docs`, `chore`, `ci`, `refactor`,
`perf`, `test`, `build`. Scopes: `core`, `cli`, `desktop`, a module id, or none.

## Code of conduct

Be kind and specific. Disagreements are about code, not people.
```

**Step 4: Write `SECURITY.md`**

```markdown
# Security

Storage Monitor deletes files and runs `git`, `docker` and `xcrun` on your
behalf, so bugs here can destroy data. Please report vulnerabilities and
data-loss bugs privately through
[GitHub private vulnerability reporting](https://github.com/vaital3000/storage-monitor/security/advisories/new)
rather than a public issue. Expect an acknowledgement within a week.

Supported: the latest release only.
```

**Step 5: Write `.github/CODEOWNERS`**

```
* @vaital3000
```

**Step 6: Write `.github/PULL_REQUEST_TEMPLATE.md`**

```markdown
## Summary

<!-- What changes and why. Link the issue or the plan task. -->

## Test plan

<!-- Commands you ran and what they showed. UI change: attach the Playwright screenshot. -->

## Checklist

- [ ] `just ci` passes
- [ ] Tests added or updated
- [ ] Docs updated (CLAUDE.md, ADR, module guide) where behavior or layout changed
- [ ] Deletion paths (if touched) respect allowed roots and Trash-by-default
```

**Step 7: Write the issue templates**

`.github/ISSUE_TEMPLATE/config.yml`:
```yaml
blank_issues_enabled: true
```

`.github/ISSUE_TEMPLATE/bug_report.yml`:
```yaml
name: Bug report
description: Something is wrong, including wrong sizes or wrong cleanup verdicts
labels: [bug]
body:
  - type: input
    id: version
    attributes:
      label: Version
      description: From the About screen or `storage-monitor info`
    validations:
      required: true
  - type: input
    id: macos
    attributes:
      label: macOS version
    validations:
      required: true
  - type: textarea
    id: what
    attributes:
      label: What happened
      description: Steps, expected result, actual result. Paste `storage-monitor ... --json` output where relevant.
    validations:
      required: true
  - type: checkboxes
    id: data
    attributes:
      label: Data loss
      options:
        - label: This bug deleted something it should not have
```

`.github/ISSUE_TEMPLATE/module_request.yml`:
```yaml
name: Module request
description: Propose a new cleanup module (a tool or cache that eats disk space)
labels: [module]
body:
  - type: input
    id: tool
    attributes:
      label: Tool or artifact type
      placeholder: e.g. Gradle caches, Android emulators, Homebrew
    validations:
      required: true
  - type: textarea
    id: where
    attributes:
      label: Where the data lives and how to measure it
      description: Paths, commands that list objects and sizes
    validations:
      required: true
  - type: textarea
    id: rules
    attributes:
      label: When is it safe to delete, and how
      description: The exact cleanup commands and what makes an object stale
    validations:
      required: true
```

**Step 8: Write the ADRs**

`docs/adr/0001-record-architecture-decisions.md`:
```markdown
# 1. Record architecture decisions

Date: 2026-09-17. Status: accepted.

## Context

The project is built with coding agents that start every session with no
memory. Decisions must be findable and their reasons preserved.

## Decision

Record significant decisions as numbered files in `docs/adr/` using this
format: context, decision, consequences. The initial product design is in
`docs/plans/2026-09-17-storage-monitor-design.md`; ADRs capture changes and
the most important choices from it.

## Consequences

Agents and contributors read `docs/adr/` before proposing a change of
direction. Superseded ADRs stay in place with a "superseded by" note.
```

`docs/adr/0002-tauri-rust-react-stack.md`:
```markdown
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
```

`docs/adr/0003-trash-first-dual-deletion.md`:
```markdown
# 3. Trash by default, permanent deletion as an explicit mode

Date: 2026-09-17. Status: accepted.

## Context

The app exists to delete things. Wrong verdicts will happen. Docker objects
and simulators have no Trash; files and folders do.

## Decision

Two modes, chosen per run and as a setting: Trash (default) and Permanent.
File-based actions move to the macOS Trash in Trash mode. Command-based
actions (`docker ... rm`, `xcrun simctl delete`) run the same in both modes
and the confirmation dialog says they cannot be undone. Every action is
planned, previewed, confirmed and logged.

## Consequences

Trash mode does not free space until the Trash is emptied; the UI says so.
Worktree removal in Trash mode is "move folder to Trash + git worktree prune"
rather than `git worktree remove`.
```

**Step 9: Format and commit**

```bash
pnpm format && pnpm format:check
git add -A
git commit -m "docs: add README, agent guide, contributing, templates and ADRs"
```

---

### Task 9: CI workflow, PR title lint, Dependabot

**Files:**
- Create: `.github/workflows/ci.yml`, `.github/workflows/pr-title.yml`, `.github/dependabot.yml`

Notes for the executor:
- On Ubuntu the Tauri crate needs webkit2gtk system libraries to compile, so
  the Rust job excludes the desktop crate. The desktop crate is checked on the
  macOS job.
- The macOS job costs 10x minutes on private repositories. It stays because
  the desktop crate must compile on every PR; if quota becomes a problem, make
  the repository public or restrict the job with `paths` filters.

**Step 1: Write `.github/workflows/ci.yml`**

```yaml
name: CI

on:
  pull_request:
  push:
    branches: [main]

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

permissions:
  contents: read

jobs:
  rust:
    name: rust
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --exclude storage-monitor-desktop --all-targets -- -D warnings
      - run: cargo test --workspace --exclude storage-monitor-desktop

  frontend:
    name: frontend
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: pnpm/action-setup@v6
      - uses: actions/setup-node@v7
        with:
          node-version: 22
          cache: pnpm
      - run: pnpm install --frozen-lockfile
      - run: pnpm format:check
      - run: pnpm --filter @storage-monitor/desktop typecheck
      - run: pnpm --filter @storage-monitor/desktop lint
      - run: pnpm --filter @storage-monitor/desktop test
      - run: pnpm --filter @storage-monitor/desktop build
      - run: pnpm --filter @storage-monitor/desktop exec playwright install --with-deps chromium
      - run: pnpm --filter @storage-monitor/desktop e2e
      - uses: actions/upload-artifact@v7
        if: ${{ !cancelled() }}
        with:
          name: playwright
          path: |
            apps/desktop/playwright-report
            apps/desktop/test-results
          retention-days: 7
          if-no-files-found: ignore

  desktop:
    name: desktop
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v7
      - uses: pnpm/action-setup@v6
      - uses: actions/setup-node@v7
        with:
          node-version: 22
          cache: pnpm
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - run: pnpm install --frozen-lockfile
      - run: cargo clippy -p storage-monitor-desktop --all-targets -- -D warnings
      - run: cargo test -p storage-monitor-desktop
      - run: pnpm --filter @storage-monitor/desktop tauri build --debug --no-bundle
```

**Step 2: Write `.github/workflows/pr-title.yml`**

```yaml
name: PR title

on:
  pull_request:
    types: [opened, edited, synchronize, reopened]

permissions:
  pull-requests: read

jobs:
  pr-title:
    name: pr-title
    runs-on: ubuntu-latest
    steps:
      - uses: amannn/action-semantic-pull-request@v6
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        with:
          types: |
            feat
            fix
            docs
            chore
            ci
            refactor
            perf
            test
            build
          requireScope: false
```

**Step 3: Write `.github/dependabot.yml`**

```yaml
version: 2
updates:
  - package-ecosystem: cargo
    directory: /
    schedule:
      interval: weekly
    commit-message:
      prefix: chore
      include: scope
    groups:
      rust:
        patterns: ['*']
  - package-ecosystem: npm
    directory: /
    schedule:
      interval: weekly
    commit-message:
      prefix: chore
      include: scope
    groups:
      npm:
        patterns: ['*']
  - package-ecosystem: github-actions
    directory: /
    schedule:
      interval: weekly
    commit-message:
      prefix: chore
      include: scope
    groups:
      actions:
        patterns: ['*']
```

**Step 4: Validate YAML locally and commit**

Run: `pnpm format:check` (Prettier validates YAML syntax) and, if `actionlint` is available, `actionlint`.

```bash
git add .github
git commit -m "ci: add CI, PR title lint and dependabot"
```

---

### Task 10: Release automation

**Files:**
- Create: `release-please-config.json`, `.release-please-manifest.json`, `.github/workflows/release.yml`

How it works: every push to `main` runs release-please, which keeps a
"chore(main): release X.Y.Z" PR up to date. Merging that PR creates the tag
and the GitHub Release in the same workflow run, and the `build-macos` job
(chained with `needs`, so no PAT is required) builds the universal `.dmg` and
the CLI and uploads them. `workflow_dispatch` rebuilds assets for an existing
tag.

Strategy: `simple`, not `rust`. release-please's Rust strategy also rewrites
the root `Cargo.toml` and throws `is not a package manifest` on a virtual
workspace root (release-please issue 1998). With `simple`, every version
location is bumped through `extra-files`: the three crate manifests, the three
`[[package]]` entries in `Cargo.lock`, and both `package.json` files.
release-please ignores a manifest version of `0.0.0`, so the first version is
set explicitly with `initial-version`.

The `Cargo.lock` rules compare `@.name.value`, not `@.name`: release-please's
TOML parser wraps every scalar as `{start, end, value}`. A rule that matches
nothing only logs a warning, so keep the `.value` form and check the release
PR diff for the three `Cargo.lock` lines.

The release PR is opened with the workflow token, so GitHub does not run CI on
it; review its diff (versions and `CHANGELOG.md` only) and close/reopen it to
force a CI run.

**Step 1: Write `release-please-config.json`**

```json
{
  "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
  "release-type": "simple",
  "initial-version": "0.1.0",
  "include-component-in-tag": false,
  "bump-minor-pre-major": true,
  "bump-patch-for-minor-pre-major": false,
  "changelog-sections": [
    { "type": "feat", "section": "Features" },
    { "type": "fix", "section": "Bug Fixes" },
    { "type": "perf", "section": "Performance" },
    { "type": "docs", "section": "Documentation" },
    { "type": "refactor", "section": "Refactoring", "hidden": true },
    { "type": "test", "section": "Tests", "hidden": true },
    { "type": "ci", "section": "CI", "hidden": true },
    { "type": "chore", "section": "Chores", "hidden": true },
    { "type": "build", "section": "Build", "hidden": true }
  ],
  "packages": {
    ".": {
      "extra-files": [
        { "type": "toml", "path": "crates/core/Cargo.toml", "jsonpath": "$.package.version" },
        { "type": "toml", "path": "crates/cli/Cargo.toml", "jsonpath": "$.package.version" },
        { "type": "toml", "path": "apps/desktop/src-tauri/Cargo.toml", "jsonpath": "$.package.version" },
        { "type": "toml", "path": "Cargo.lock", "jsonpath": "$.package[?(@.name.value=='storage-monitor-core')].version" },
        { "type": "toml", "path": "Cargo.lock", "jsonpath": "$.package[?(@.name.value=='storage-monitor-cli')].version" },
        { "type": "toml", "path": "Cargo.lock", "jsonpath": "$.package[?(@.name.value=='storage-monitor-desktop')].version" },
        { "type": "json", "path": "package.json", "jsonpath": "$.version" },
        { "type": "json", "path": "apps/desktop/package.json", "jsonpath": "$.version" }
      ]
    }
  }
}
```

**Step 2: Write `.release-please-manifest.json`**

```json
{
  ".": "0.0.0"
}
```

The first release PR proposes `0.1.0` because of `initial-version`; later
releases follow conventional commits (`feat` bumps minor while below 1.0).

**Step 3: Write `.github/workflows/release.yml`**

```yaml
name: Release

on:
  push:
    branches: [main]
  workflow_dispatch:
    inputs:
      tag:
        description: 'Existing release tag to build assets for (e.g. v0.1.0)'
        required: true
        type: string

permissions:
  contents: write
  pull-requests: write
  issues: write

concurrency:
  group: release
  cancel-in-progress: false

jobs:
  release-please:
    if: github.event_name == 'push'
    runs-on: ubuntu-latest
    outputs:
      release_created: ${{ steps.rp.outputs.release_created }}
      tag_name: ${{ steps.rp.outputs.tag_name }}
    steps:
      - id: rp
        uses: googleapis/release-please-action@v5
        with:
          config-file: release-please-config.json
          manifest-file: .release-please-manifest.json

  build-macos:
    needs: release-please
    if: ${{ !cancelled() && (github.event_name == 'workflow_dispatch' || needs.release-please.outputs.release_created == 'true') }}
    runs-on: macos-latest
    env:
      TAG: ${{ github.event_name == 'workflow_dispatch' && inputs.tag || needs.release-please.outputs.tag_name }}
    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ env.TAG }}
      - name: Verify the release exists
        env:
          GH_TOKEN: ${{ github.token }}
        run: gh release view "$TAG" --json tagName --jq .tagName
      - uses: pnpm/action-setup@v6
      - uses: actions/setup-node@v7
        with:
          node-version: 22
          cache: pnpm
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-apple-darwin,x86_64-apple-darwin
      - uses: Swatinem/rust-cache@v2
      - run: pnpm install --frozen-lockfile

      - name: Build universal app bundle and DMG
        run: pnpm --filter @storage-monitor/desktop tauri build --target universal-apple-darwin --bundles dmg

      - name: Build universal CLI
        run: |
          cargo build --release -p storage-monitor-cli --target aarch64-apple-darwin
          cargo build --release -p storage-monitor-cli --target x86_64-apple-darwin
          mkdir -p dist
          lipo -create -output dist/storage-monitor \
            target/aarch64-apple-darwin/release/storage-monitor \
            target/x86_64-apple-darwin/release/storage-monitor
          codesign --sign - --force dist/storage-monitor

      - name: Package assets
        run: |
          set -euo pipefail
          VERSION="${TAG#v}"
          mv target/universal-apple-darwin/release/bundle/dmg/*.dmg "dist/StorageMonitor-${VERSION}-macos-universal.dmg"
          tar -C dist -czf "dist/storage-monitor-cli-${VERSION}-macos-universal.tar.gz" storage-monitor
          rm dist/storage-monitor
          (cd dist && shasum -a 256 * > checksums.txt)
          ls -la dist

      - name: Upload assets to the GitHub release
        env:
          GH_TOKEN: ${{ github.token }}
        run: gh release upload "$TAG" dist/* --clobber
```

**Step 4: Commit**

```bash
pnpm format:check
git add release-please-config.json .release-please-manifest.json .github/workflows/release.yml
git commit -m "ci: add release-please and macOS release build"
```

---

### Task 11: Open the PR, get CI green, merge

**Step 0: Repository settings that the release flow depends on**

Run once (they are not files in the repository):
```bash
gh api -X PUT repos/vaital3000/storage-monitor/actions/permissions/workflow -f default_workflow_permissions=read -F can_approve_pull_request_reviews=true
gh api -X PUT repos/vaital3000/storage-monitor/private-vulnerability-reporting
gh api -X PATCH repos/vaital3000/storage-monitor -F allow_merge_commit=false -F allow_rebase_merge=false -F delete_branch_on_merge=true
```
The first one lets release-please open its PR with the workflow token; the
second backs the link in `SECURITY.md`; the third enforces squash-only merges
and removes merged branches.

**Step 1: Final local gate**

Run: `just ci`
Expected: all green.

**Step 2: Push and open the PR**

```bash
git push -u origin feat/phase-0-walking-skeleton
gh pr create --title "feat: phase 0 walking skeleton" --body-file - <<'PR'
## Summary

Walking skeleton for Storage Monitor (phase 0 of the design):

- Cargo workspace: `storage-monitor-core`, `storage-monitor-cli`, `storage-monitor-desktop`
- Tauri 2 app with one command (`get_app_info`) and a React screen showing the version
- Frontend mock mode (`vite --mode mock`) used by Vitest and Playwright
- `just` recipes, ESLint, Prettier, rustfmt, clippy
- CI (Rust on Ubuntu, frontend + e2e on Ubuntu, Tauri build smoke on macOS), PR title lint, Dependabot
- release-please + macOS release build that attaches a universal `.dmg` and the CLI to GitHub Releases
- README, CLAUDE.md/AGENTS.md, CONTRIBUTING, SECURITY, templates, ADRs 1-3

## Test plan

- `just ci` locally
- CI on this PR
- After merge: release-please opens the `0.1.0` PR; merging it must produce a release with `StorageMonitor-0.1.0-macos-universal.dmg`, the CLI tarball and `checksums.txt`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
PR
```

**Step 3: Watch CI**

Run: `gh pr checks --watch`
Expected: `rust`, `frontend`, `desktop`, `pr-title` all pass. Fix failures on the branch; do not merge red.

**Step 4: Self-review, then merge**

Use superpowers:requesting-code-review for a review pass on the diff, address findings, then:

```bash
gh pr merge --squash --delete-branch
```

---

### Task 12: First release `v0.1.0` and verification

**Step 1: Wait for the release PR**

Optional preview before the workflow runs (needs a GitHub token):
```bash
npx release-please release-pr --dry-run --repo-url=vaital3000/storage-monitor --token="$(gh auth token)" --config-file=release-please-config.json --manifest-file=.release-please-manifest.json
```

After the merge, the `Release` workflow on `main` opens a PR titled
`chore(main): release 0.1.0`. GitHub does not run CI on it (it is opened by
the workflow token); review the diff instead. Check with:

```bash
gh pr list --search "chore(main): release" --state open
```

Open it and verify the diff bumps: `crates/core/Cargo.toml`,
`crates/cli/Cargo.toml`, `apps/desktop/src-tauri/Cargo.toml`, `Cargo.lock`,
`package.json`, `apps/desktop/package.json`, and creates `CHANGELOG.md`.
If a file is missing from the bump, fix `release-please-config.json` on a
branch, merge it, and wait for the release PR to refresh.

**Step 2: Merge the release PR**

```bash
gh pr merge <number> --squash
```

**Step 3: Watch the release build**

```bash
gh run list --workflow=release.yml --limit 3
gh run watch <run-id>
```
Expected: `build-macos` succeeds and uploads three assets.

**Step 4: Verify the release**

```bash
gh release view v0.1.0 --json assets --jq '.assets[].name'
```
Expected:
```
StorageMonitor-0.1.0-macos-universal.dmg
checksums.txt
storage-monitor-cli-0.1.0-macos-universal.tar.gz
```

Download and check locally:
```bash
cd /tmp && gh release download v0.1.0 -R vaital3000/storage-monitor -p '*.dmg' -p '*.tar.gz' --clobber
hdiutil attach StorageMonitor-0.1.0-macos-universal.dmg -nobrowse -mountpoint /tmp/sm-dmg
codesign -dv "/tmp/sm-dmg/Storage Monitor.app" 2>&1 | grep -E 'Signature|Identifier'
lipo -archs "/tmp/sm-dmg/Storage Monitor.app/Contents/MacOS/"*
hdiutil detach /tmp/sm-dmg
tar xzf storage-monitor-cli-0.1.0-macos-universal.tar.gz && ./storage-monitor info --json
```
Expected: `Signature=adhoc`, `lipo` prints `x86_64 arm64`, the CLI prints
`"version": "0.1.0"`.

**Step 5: Record the outcome**

Update the status line in `README.md` if needed and note any deviations from
this plan in `docs/plans/2026-09-17-phase-0-walking-skeleton.md` under a
final "Outcome" heading (one short paragraph), via a small `docs:` PR.

---

## Out of scope for phase 0 (tracked in the design backlog)

- Custom app icon (phase 6).
- Claude GitHub Action for PR review (needs a secret; maintainer decision).
- Branch protection rules (GitHub requires a public repository or a paid
  plan; enable when the repository goes public).
- Homebrew tap and notarization.

---

## Outcome (2026-09-17)

Phase 0 shipped as [`v0.1.0`](https://github.com/vaital3000/storage-monitor/releases/tag/v0.1.0)
on the day it was planned: PR #3 (walking skeleton) and PR #4 (release-please)
were squash-merged, and `release.yml` attached the universal `.dmg`, the CLI
tarball and `checksums.txt`. Verified locally: checksums match, the app is
ad-hoc signed and universal, `LSMinimumSystemVersion` is 13.3, the CLI prints
`0.1.0`.

Deviations from the original plan, all folded back into the text above:

- release-please runs the `simple` strategy with TOML `extra-files` (the Rust
  strategy fails on a virtual workspace root), and the `Cargo.lock` rules use
  `@.name.value`.
- The release PR does not trigger CI; with branch protection on, it is closed
  and reopened once to get the required checks.
- Repository settings had to be flipped: Actions may create PRs, squash-only
  merges, delete branch on merge, private vulnerability reporting, and branch
  protection on `main` with the checks `rust`, `frontend`, `desktop`, `pr-title`.
- TypeScript stayed at the scaffold's 6.x; `tsc` checks only `src/`.
- Minimum macOS is 13.3 (Tailwind v4 needs Safari 16.4); the desktop crate is
  `rlib`-only; CI job names are the short `rust`/`frontend`/`desktop`.
- The live Tauri window was launched and ran, but could not be screenshotted
  on a locked screen; the IPC round trip is covered by the unit test and the
  mocked e2e.
