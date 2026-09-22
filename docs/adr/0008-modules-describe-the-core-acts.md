# 8. Modules describe, the core acts

Date: 2026-09-22. Status: accepted. Supersedes the `Module` trait and `ActionSpec` of
design section 7.

## Context

Design section 7 gave a module
`async fn execute(&self, req: &ActionRequest, sys: &dyn System) -> ActionResult`: the
module would remove what it found, with the port in its hands. Section 9 of the same
design promises guards "enforced in the core regardless of module". Both cannot be true
at once. A module holding `System::remove` and `System::run` can reach any path and run
any command, and the core would learn what it did from the result, after the fact.

The same section declared the trait async. Nothing else in the core is: the walker runs
on rayon, the action engine is a loop, the desktop runs both on blocking threads, and the
CLI has no async runtime at all.

## Decision

1. A module has no `execute`. It answers
   `plan(item, action, options, mode) -> Vec<Step>`, and the core runs the steps. A step
   deletes a path — to the Trash or for good, as the batch's mode says — or runs a
   command whose effect the module declares: housekeeping, destroying something outside
   the filesystem, or removing a path. Every path a step deletes, directly or through a
   command, passes `Limits::check` in the preview and again right before the step.
2. `plan` takes no `System`. Planning cannot touch anything.
3. The trait is synchronous, and discovery runs on a thread per module.
4. `ActionSpec` loses `danger`, `supports_trash`, `reversible` and `preview`. They are
   properties of the steps an action plans in a given mode, and the core computes them.
5. A Keep verdict is enforced in the core: the item is refused unless one of the action's
   `force` options is on.
6. Module crates live under `crates/modules/`, where a `clippy.toml` disallows the calls
   of the standard library that delete or start a process.

## Consequences

A module cannot walk around the guards, the preview, the acknowledgement or the record,
because none of them is its code. The dialog's acknowledgement is always about the steps
it shows, since reversibility is derived from them.

Some freedom is lost. A module cannot branch on the result of a step — run A, then B only
if A printed X. None of the three v1 modules needs to; when one does, the vocabulary of
steps grows in the core, next to the guards. A module that needs a new kind of effect
needs a change in the core, and that is the point.

Discovery still runs arbitrary module code with the port in hand, and nothing but review
stops it from running `docker image rm` while it looks around. The line is drawn where it
can be held: every destructive command the app runs on the user's behalf is one the user
saw in the preview.
