# 4. Snapshot format: postcard + lz4 payload with a JSON sidecar

Date: 2026-09-18. Status: accepted.

## Context

Every completed scan is persisted so that the next scan of the same folder can
show what grew (design section 6.2). A snapshot of a home folder holds a few
hundred thousand directories plus the files worth listing. It must be quick to
write at the end of a scan and quick to read at the start of the next one, and
listing the store must not require decoding payloads. The only query in phase 1
is "the previous snapshot of this root".

## Decision

A snapshot is a Rust struct (`crates/core/src/snapshot/model.rs`) serialized
with `postcard` and compressed with `lz4_flex`, written as `<stamp>.snap`, next
to a `<stamp>.json` sidecar with what listing needs: format, `takenAt`, root,
total bytes and file count. The stamp is the UTC start time of the scan
(`20260918T084134.352Z`).

The entries are every directory plus the files whose allocated size is at least
the threshold (10 MiB in the app and by default in the CLI), keyed by absolute
path, in depth-first order with children largest first: the entries of one
subtree share their prefix and sit next to each other, which compresses well.
One root per snapshot. The desktop app and the CLI share the store in
`~/Library/Application Support/storage-monitor/snapshots/`.

The store (`crates/core/src/snapshot/store.rs`) keeps the 10 newest snapshots
across all roots. Each file is written to `*.tmp` and renamed into place, the
sidecar last, so a snapshot is listed only once it is complete. `prune` also
removes damaged sidecars together with their payloads, orphan payloads and
sidecars, and leftover `*.tmp` files.

Not SQLite, not yet: there is no query beyond "the newest snapshot of this
root", a home-folder snapshot (600,000 entries) is 10 to 12 MB on disk and
decodes in about 50 ms on an Apple Silicon Mac, and a database engine would buy
nothing at this size. It can replace the files when the Overview needs history
across snapshots.

## Consequences

The format version (`SNAPSHOT_FORMAT`) is the first field of the payload and a
field of the sidecar. A layout change bumps it; files of another format are
ignored by `list` and left alone by `prune`, so an older or newer build never
misreads or destroys them. Deltas are computed by absolute path, so a renamed
folder shows as removed plus added, and files below the threshold never appear
in the deltas. A CLI `scan --save` becomes the baseline of the next desktop scan
of the same root. Tests point the store at a temp dir (`STORAGE_MONITOR_DATA_DIR`
or `ScanManager::with_snapshots_dir`).
