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
