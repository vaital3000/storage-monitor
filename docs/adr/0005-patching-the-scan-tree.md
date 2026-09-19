# 5. Patching the scan tree: rebuild the arena, bump the generation

Date: 2026-09-20. Status: accepted.

## Context

A scan of a home folder takes about 25 seconds. After deleting a few entries
the Explorer must stop showing them, and rescanning the root to achieve that
would make every deletion cost a full scan. Rescanning the parent is no better:
a folder deleted directly under `~` has the home folder as its parent.

So a batch rescans only the paths it touched (design decision 4) and splices
the results into the tree already in memory. The tree is an arena: a `Vec<Node>`
in breadth-first order, where the children of a node occupy a contiguous
`Range<NodeId>` sorted largest-first. Contiguity is what makes a child lookup a
slice rather than a search, and it is also what makes in-place surgery
impossible — removing one child would have to shift every id after it.

## Decision

`Tree::replace_subtrees` rebuilds the arena rather than editing it, and
`ScanManager::install_patches` bumps the generation when it installs the
result. **A `NodeId` does not survive a generation.** Any reader holding ids
from before the splice is reading a tree that no longer exists.

The rebuild re-aggregates the ancestors of every patch and sorts again the
sibling groups that changed; everything else keeps its order. The root cannot
be patched.

The generation is `Inner.generation`, and it never crosses the wire. The UI
counts its own generations from `scan:done`; the two are different numbers with
different jobs. The backend's stops one patch being spliced onto an arena that
another patch already replaced (`patch_paths` step 3 re-checks it under the
lock). The front-end's is a cache key.

## Consequences

The invariant is one sentence, and during phase 2a it escaped through four
different doors. They are listed because the next one will not look like the
first four either.

1. **The splice did not bump the generation.** Found in Task 7. Two batches in
   flight, and the second installs onto an arena the first replaced.
2. **The cache was keyed by the front-end generation**, which `scan:done` bumps
   and a splice does not — so after a batch every `['treeNode', generation, id]`
   entry still held ids from the discarded arena, and the Explorer went on
   rendering a tree that no longer existed. The cure is to invalidate the tree
   queries and the disk usage query after a batch, without bumping the
   generation: the scan did not change.
3. **The table's range anchor was keyed on `node.id`**, and the root is always
   `ROOT_ID` — so the anchor survived a rescan that renumbered everything under
   it. Found in Task 12.
4. **Invalidating the cache fixes the cache, not the key.** Found in Task 14,
   and the subtlest of the four: the refetch goes out with the id of the
   directory the user is _in_, resolved against the replaced arena. It is not
   only deleted rows that move. Every ancestor of a patch shrank, so their
   sibling groups sort again, and a folder that drops below a sibling hands
   over its id. The refetch then opens a different directory and labels it with
   another directory's breadcrumbs — no error, no empty state, just the wrong
   folder. The page therefore re-anchors on `ROOT_ID` after any batch that
   patched anything, which is exactly the batches for which `touched()` hands
   the splice a non-empty list: `Removed` or `Failed`, never `Skipped`.

What ties them together is that each door is a _different_ place where an id is
kept across time: a worker thread, a query cache, a component ref, a fetch
argument. Reviewing "does this bump the generation?" catches only the first.
The question that catches all four is "where is a `NodeId` stored, and what
happens to it while it is stored there?"

A cost that stays: `invalidateQueries` marks entries stale but keeps serving
them, so a directory the user navigates back to renders old ids for one round
trip. Nothing can be deleted wrongly — a batch carries absolute paths, not ids —
but a click in that window is door 4 one hop removed.

Rebuilding is fast enough to be uninteresting: sorting every sibling group
instead of only the changed ones is also correct and measured at 96-105 ms over
3.7 M nodes either way. The optimisation is an optimisation, not a rule.
