# Media lab — contract shell (zenpipe#113, slice 1)

First slice of `docs/media-suite.md`: a deterministic orchestration shell,
not a codec implementation. Everything here is MODEL evidence — it
qualifies the task contract only, never codec correctness, memory
performance, or native-WASM behavior.

## Files

- `index.html`, `main.mjs` — accessible single-page shell: generated
  fixture, task controls, preview canvas, receipts log, prominent `MODEL`
  badge, link back to `../index.html` (the editor).
- `controller.mjs` — the internal deterministic controller. Tasks carry
  operation ID + source generation + backend identity through
  `queued → admitted → running → completed|failed|cancelled` (with
  `cancelling` between a cancel signal and the worker's terminal
  receipt). Fixed global CPU tokens and declared byte reservations —
  accounting only, never claimed as measured peaks or proven bounds;
  strict admission refuses undeclared reservations; impossible
  reservations refuse instead of queueing forever.
- `backend-model.mjs` — deterministic async model backend with the
  submit/cancel/dispose + tagged-event surface a native adapter will
  later translate real operation receipts onto. The scheduler is
  injectable so tests drive it deterministically.
- `fixture.mjs` — closed-form generated RGBA fixture and the trivial
  model ops (invert, swap-rb, fade). Every pixel is a function of
  (x, y, seed); `seed` is the source generation.

## Contract points exercised

- Queued cancellation has zero backend effects; running/admitted
  cancellation signals the backend and holds the reservation until the
  terminal receipt — `cancel-ack` is never counted as exit.
- Superseded completions — an older source generation, or any op that is
  not the latest requested one — are retained as bounded summaries for
  diagnostics and release their reservation, but cannot update the
  preview; raw pixels are never retained.
- Duplicate/stale terminal events never double-release or change state;
  events for queued tasks and foreign-identity `backend-error`s are
  untrusted diagnostics, not effects.
- `backend.submit` has an explicit contract: `SubmitRefusal` = guaranteed
  no effects; any other throw after events = retained ownership; any
  other throw with no events = uncertain start (cancelling + cleanup
  signal, reservation held until terminal).
- A `backend-error` event settles every task that backend owns, and the
  dead backend refuses further submissions rather than queueing forever.
- `teardown()` cancels owned tasks and reports unresolved ownership and
  held reservations honestly; a silent worker leaves tasks unresolved.
- Estimate (empirical, or `unknown` — never zero) and declared
  reservation are displayed as distinct values; observed peak is
  honestly "not measured". A payload's caller memory sits outside the
  declared reservation while queued; it is handed to the backend on
  admission or dropped on refusal/cancel — no whole-memory bound claim.
- Observer callbacks cannot interrupt transitions: faults are recorded
  (`observerFaults()`), never thrown into the machine; delivery is
  re-checked after settlement so a mid-settle generation advance or a
  newer op cannot receive stale output.
- Unsupported media kinds (AVIF sequence, MP4/VP8 clips, TIFF collection,
  JXL layers) refuse as unqualified before any effect.

## Checks

`just media-shell-check` — Node built-in tests against a scripted fake
backend (and the real model backend with a manual scheduler), then the
Playwright spec (`demo/tests/media-suite.spec.js`) for controls, badge,
keyboard, 390 px overflow and exact fixture pixels. The browser leg needs
`npm install` in `demo/` and fails loudly without it.

## Deliberately out of scope

No Rust/public API, no WASM, no Pulse/shared-cell cancellation bridge, no
native adapter, no collection/video decode, no measured scheduling. Model
cancellation is async-cooperative only; nothing claims a postMessage
interrupts synchronous WASM.
