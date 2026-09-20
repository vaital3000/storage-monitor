# 6. Content Security Policy: closed, and held by a test that reads the config

Date: 2026-09-20. Status: accepted.

## Context

Phase 1 shipped with `"csp": null`. That was tolerable while the app only read
the disk; phase 2a deletes files, so the window that can be talked into
fetching a script is now a window that can be talked into deleting a home
folder.

The policy has to be decided once and then held, and holding it is the hard
part: **nothing in the ordinary development loop runs the app under it.** Tauri
attaches the header in the `tauri://localhost` handler, which serves the
embedded frontend; a `devUrl` document is served by Vite and never passes
through that handler, so `just dev` cannot apply it (`devCsp` does not help —
it is the same handler). CI builds the app but never launches it. Vitest and
Playwright run in a plain browser. Before this was addressed, deleting the
`csp` line left every gate green.

## Decision

The window ships with:

```
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self'; font-src 'self'; connect-src 'self' ipc: http://ipc.localhost;
object-src 'none'; base-uri 'self'; form-action 'none'; frame-ancestors 'none'
```

`the_content_security_policy_stays_closed` (`src-tauri/src/lib.rs`) parses
`tauri.conf.json` and holds three things: the sources of each directive that
carries weight, that no source anywhere is other than a quoted keyword, Tauri's
`ipc:` or a host under `localhost`, and — the one that is easy to miss — **the
set of directive names, as a closed set**. A list checked item by item cannot
notice an addition, and here the addition is the attack: `script-src-elem` does
not add to `script-src`, it _replaces_ it for `<script>` elements, so an
eleventh directive defeats the tenth while every assertion above it still
passes. Same shape for `style-src-attr` over `style-src`.

The reasons live in the test's doc comment and in this ADR, not beside the
line, because `tauri.conf.json` is plain JSON and `tauri-build` rejects the
config outright on an unknown key — a `"_csp"` note next to `"csp"` does not
build.

## Consequences

**`style-src` keeps `'unsafe-inline'`, and it is not for Tailwind.** Tailwind
v4 compiles at build time and injects nothing at runtime. The consumers are
inline style _attributes_: `DiskUsageBar`, `NodeTable`'s `style={{ width }}`
for column widths, and ECharts' tooltip `cssText`. They inherit the grant
through `style-src-attr`, and they break in the packaged app only — which is
exactly the class of failure no test in this repo can see.

What makes that grant tolerable is the two directives beside it. `img-src
'self'` leaves an injected `url()` nowhere to beacon to, and `font-src 'self'`
closes the same door for `@font-face`. The grant buys an attacker styling, not
exfiltration.

**`img-src` is narrow because the grants it dropped were dead, measured.** It
carried `data:`, `asset:` and `http://asset.localhost`; the app has no `<img>`,
the built CSS has no `url(`, and nothing calls `convertFileSrc`. They come back
with the feature that needs them, not before.

**`form-action 'none'` is the one directive with no fallback.** `default-src`
backstops the directives absent from this policy; `form-action` is not among
them, so removing the line opens a `<form action="https://…">` out of an
otherwise sealed policy.

**`require-trusted-types-for` is left out deliberately.** The policy is
enforced by WKWebView, which does not implement Trusted Types, so on the only
platform this app ships to the directive would enforce nothing — while
enlarging the set of directive names the test exists to keep closed. Revisit it
if the app ever ships on a Chromium-based runtime.

**Verification needs a positive control, always.** "No violations in the
console" is indistinguishable from "no policy at all", and the app spent its
first weeks in exactly that state. The policy was verified by running the built
app (`pnpm tauri build --debug --no-bundle`) with two planted controls: an
`eval`, which was blocked, and a form posting to `https://example.invalid/`,
which raised `form-action` — the directive failing where it was added to fail.
The app still worked under it: the treemap canvas painted, the stylesheet
applied, the scan ran.

**What the guard test does and does not prove.** It proves the line is still
there and still closed. It cannot prove the app runs under it, because nothing
automated launches the app. The Playwright suite is the nearest available
substitute: its mock server reads this same policy out of `tauri.conf.json` and
serves it on port 1430, so the served header cannot drift from the shipped one,
and every spec runs under it. That substitute is weaker than it looks — the
origin differs, so `'self'` does not mean there what it means in the packaged
app, and the module graph is Vite's rather than the bundle's. What transfers is
the class that actually breaks: `'unsafe-eval'`, a `blob:` worker, a `data:`
font. That is worth having, because `echarts` is a caret range — a `pnpm update`
pulling a chart build that reaches for a blob worker would ship an app whose
treemap silently fails to render, with every other gate green.

Its own positive control plants all four classes and asserts they did not merely
raise an event but failed to run. Two traps are recorded there, both measured.
A control written as `page.evaluate(() => eval(…))` proves nothing: the eval
runs, raises no violation, and reports success, because `page.evaluate` executes
through the debugger, which the page's policy does not cover — `setTimeout('…',
0)` scheduled from inside it does raise `script-src <- eval`, because the page's
own task does the compiling. And an alarm nobody trips is indistinguishable from
one that cannot fire, so a `test.fail()` spec plants a violation without
declaring it, and passes only by failing.

Serving the policy also costs the dev server its fast refresh: the React
plugin's preamble is an inline `<script>`, which `script-src 'self'` blocks, and
the app then never mounts. That is why the e2e server turns fast refresh off and
why `just dev` cannot be made to apply the policy by setting a header.

Loosening the policy means editing that test, deliberately, which is the point.
