# Storage Monitor

[![CI](https://github.com/vaital3000/storage-monitor/actions/workflows/ci.yml/badge.svg)](https://github.com/vaital3000/storage-monitor/actions/workflows/ci.yml)

A macOS disk space analyzer that knows what your developer artifacts are and
can clean them up safely: git worktrees, Docker images and caches, Xcode
DerivedData and simulators.

Most analyzers answer "where is the space". Storage Monitor answers "what can I
free right now, and why", with a preview and a confirmation before anything is
touched. Deletion goes to the Trash by default.

> Status: pre-alpha. Phase 0 (walking skeleton) is done; the scanner and the
> modules are being built. See the
> [roadmap](docs/plans/2026-09-17-storage-monitor-design.md#14-roadmap).

## Install

Download the latest `.dmg` from
[Releases](https://github.com/vaital3000/storage-monitor/releases), open it and
drag the app to Applications. Requires macOS 13.3 or newer.

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
just dev-web # run only the UI in a browser with a mocked backend
just ci      # lint + unit tests + e2e, same as CI
```

Layout, conventions and the module contract are described in
[CLAUDE.md](CLAUDE.md) and [docs/](docs/). Design decisions live in
[docs/adr](docs/adr).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Issues and pull requests are welcome.

## License

[MIT](LICENSE)
