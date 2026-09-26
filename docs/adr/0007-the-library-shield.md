# 7. `~/Library` is shielded, not denied

Date: 2026-09-21. Status: accepted. Supersedes the `~/Library` rule of design section 9.

## Context

Phase 2a shipped with `~/Library` denied wholesale. Design section 9 wrote that rule
as "`~/Library` except paths a module declares as managed", so the carve-out was
always part of it — but the module framework is phase 2b, and until then the
exception has nobody to declare it.

Meanwhile the folder is where the space is. On a developer's Mac it holds dozens of
top-level entries and tens of gigabytes, and a good part of that is the kind of thing
a disk tool exists to find — `Caches`, `Developer` (Xcode's `DerivedData`), `Logs`,
the stores of package managers. None of it was reachable:
the Explorer scans the home folder, the denylist refused every row under `Library`,
and the escape that does exist — pointing the scan root at a denied folder unlocks
it — needs a root picker, which is also phase 2b. The xcode module of phase 5 would
have met the same wall.

The obvious fix, replacing the wholesale deny with a list of the dangerous
subfolders, ages badly in exactly this folder. With dozens of entries and third-party
applications adding more, a rule of "everything not listed is deletable" is a rule
that quietly widens every time something installs itself.

## Decision

Two lists, with two different meanings, replacing one:

- **`denied`** — the path and everything below it. Unchanged rule (6), new contents.
- **`shielded`** — the path itself, and nothing else. New rule (7), an equality where
  rule 6 is a containment. Rule 6 runs first, so a denied name inside a shield wins.

`~/Library` moves from the first list to the second, together with the three folders
of application data under it — `Application Support`, `Containers`,
`Group Containers`. Sixteen names under `~/Library` stay denied outright:
`Accounts`, `Application Scripts`, `Autosave Information`, `Calendars`,
`CloudStorage`, `Contacts`, `Cookies`, `IdentityServices`, `Keychains`, `Mail`,
`Messages`, `Mobile Documents`, `Photos`, `Preferences`, `Reminders`, `Safari`.

A ninth `BlockReason`, `Shielded`, carries the verdict. It is not folded into
`Denylisted` because it is the only refusal that leaves the user somewhere to go:
the other eight say _no_, this one says _not like this — open it and choose what
inside_. A screen that said "this app never deletes from here" about `~/Library`
would be telling the user to stop in front of the gigabytes they came for.

## Consequences

**The unlock is the whole point, and it is one line of it.** Moving `~/Library`
alone from `denied` to `shielded` is what makes `Caches`, `Developer`, `Logs` and
the package-manager stores ordinary deletable rows. The three shielded folders under
it are the same trade one level down: most of the folder's bytes live in them, they
are refused as a single tick because losing all of an application's data at once is
expensive, and they are cleaned the way anyone would actually clean them — one
application at a time.

**Two of the denied names are denied for a reason the rest do not share.**
`Mobile Documents` is iCloud Drive and `CloudStorage` is where Dropbox, OneDrive and
Google Drive mount their file providers. Both were measured to sit on the data
volume like ordinary directories — `df` reports the same `/dev/disk3s5` as
everything else — so nothing structural tells them apart and a name list is the only
instrument available. They matter more than the others because **the Trash does not
undo them**: moving a file-provider item to the Trash _is_ the deletion, and it
happens on every device signed into that account. Trash-by-default (ADR 0003) is the
safety net under every other verdict in this app, and this is the one place where it
is not there.

**The list ages in the safe direction, which is why it is a shield and not a
narrower denylist.** When macOS or an installer adds one more folder to
`~/Library`, it arrives deletable — as a row inside a shield, judged on its own, and
never able to take `~/Library` with it. The failure mode of the rejected design is
the opposite one: a new folder inherits "not listed, therefore safe".

**What this does not protect, stated rather than implied.** `Group Containers` is
shielded, and one level inside it lives `group.com.apple.notes` — the Apple Notes
database. The same shape holds for `Containers`. Enumerating application data inside
those folders was considered and rejected: the list is unbounded, it would need an
entry for every application ever installed, and its real cost is the false
confidence that anything missing from it is safe. What protects a row here is what
protects every other row — it is ticked by hand, the dialog names the path, and the
mode is Trash.

**Naming a root no longer reaches the denied names.** `Limits::new` drops a denied
entry that contains the scan root, so pointing the root at a denied folder unlocks
it; that stays, and it is what lets a `~/Library` root hand out `Caches`. But the
sixteen names are _inside_ such a root rather than above it, so the drop never
reaches them and they stay denied wherever the root is pointed. That is a deliberate
strengthening: the old rule was a navigation guard — do not wander where you did not
mean to go — and these names are not about navigation. Losing one is not
recoverable, and no choice of root makes it so.

**The rule exists twice and is held in one place.** `crates/core/src/action/guards.rs`
and the desktop mock's `checkPath` both implement the seven rules, and
`crates/core/tests/fixtures/guard-cases.json` is the cases both of them answer — so
a rule that changes on one side reddens a test instead of drifting. The shared cases
carry the positive control the whole decision rests on: `~/Library/Caches` is
`ready`. A shield that took its contents with it would be a denylist under another
name and would pass every negative case above it.
