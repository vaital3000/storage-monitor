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
