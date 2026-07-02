# HANDOFF — Ghost Protocol: background index hydrate + demand-walk memo + NEDB v2.6.0

**Date:** 2026-07-02 · **Authors:** Vex (Claude Fable 5) × Mark (Interchained) · **Branch:** `hyperagent/2026-07-02-ghost-background-hydrate`

## The symptom (measured, Mark's iMac boot log 2026-07-02)

Same build, same chain (~507k blocks), `-dagv3 -dagfastsync -ghost-protocol`:

| Phase | Windows (Nemo) | Intel iMac |
|---|---|---|
| Warm-boot window (2016 headers) | ~2s total boot | **99s (~20/s)** |
| Demand-load walk tip→genesis | 464k in ~13s (**~35,700/s**) | **~40/s steady → ~3.5h projected** |

`-dagfastsync` was ON in the log — fsync is NOT the cause. 40/s is a **disk-seek number**: each demand-load did 2 random NEDB point reads; HDD/Fusion-class media delivers ~100 random IOPS ÷ 2.5 reads ≈ 40/s. NVMe forgives random I/O; spinning media does not. The walk also ran **synchronously inside whichever GetAncestor caller needed a deep parent** — this build's banner said so honestly ("hydrate still synchronous").

Trigger vs root cause: the trigger is any deep GetAncestor during header sync; the root cause is a random-I/O one-at-a-time hydrate on the caller's critical path.

## What shipped (Stage 1 — improve `-ghost-protocol`, no new engine surface)

1. **Demand-walk memo** (`src/validation.cpp`, `WarmBootLoadParent`) — the walk re-read the *child's* record every step just to learn `hashPrev`, but that record was read one step earlier to populate it. One-entry memo (guarded by `cs_main`, keyed by exact hash, correctness never depends on it) → **2 reads per ancestor becomes 1**. Expected: iMac walk ~40/s → ~80/s; Nemo proportionally.

2. **Background index hydrate** (`src/validation.cpp` + `src/ghost.h` + `src/init.cpp`) — `GhostStartBackgroundIndexHydrate()`, spawned right after INSTANT BOOT. Walks tip→genesis through the **existing** `WarmBootLoadParent` chokepoint on a dedicated thread, in chunked `cs_main` holds (256/chunk, 1ms breather), cooperating with the on-demand path through the same map (whoever loads a parent first wins; the other skips free). Wires the until-now-unused states: `Advance(IndexHydrating)` at start, `SetHydratedThrough(tip)` + `Advance(IndexReady)` at genesis. Exits within one chunk of shutdown; joined in `Shutdown()` before chainstate teardown; try/catch so a background optimization can never take the daemon down. **The boot-blocking crawl becomes a background fill; the node serves immediately either way.**

3. **NEDB pin `v2.5.0 → v2.6.0`** (`nedb-ffi/Cargo.toml`) — itcd was fifteen engine releases behind. Gains: cached segment read handles + positional reads (pread) on every demand-load/chainstate point read (+23% v3 point reads measured on VPS; larger expected where `CreateFile`/open dominates), plus the 2.5.55 correctness batch — **id-index WAL flush race under same-key churn (the UTXO shape), seq-guarded durable tips, MANIFEST fsync ordering, NQL LIMIT truncation, batch sorted-index parity, cold-scan seq off-by-one**.

## Expected iMac outcome (engineering estimate — verify by boot log)

- Walk rate roughly doubles (memo) plus pread trim → and it all runs **in the background**: header sync + restricted serving no longer wait on it.
- The familiar `WarmBoot: demand-loaded N ancestor(s)` lines keep printing (same counter, now driven by the hydrate thread), ending with `[GHOST] background index hydrate COMPLETE — N ancestors in Xs (Y/s)`.

## What this is NOT (Stage 2, queued)

The full fix for seek-bound media is a **sequential segment sweep**: NEDB-side `for_each_object_sequential()` (segment locations sorted by (segment, offset) = physically sequential reads; the streaming-cold-scan shape from the NEDB v2.5.45 review), one FFI export, then this thread swaps its read loop — random-seek hydrate becomes a ~30s sequential read even on HDD. That lands in NEDB first, deliberately, next turn. This PR's thread is the permanent home it plugs into.

## Verify

```
./interchainedd -dagv3 -dagfastsync -ghost-protocol ...
# watch for:
#   [GHOST] anchor-pending / anchored
#   [GHOST] * -> index-hydrating          <- new
#   WarmBoot: demand-loaded ... (same lines as before, now background)
#   [GHOST] background index hydrate COMPLETE — ... (/s)   <- new
#   [GHOST] * -> index-ready              <- new
# node serves headers/RPC while those tick.
```

© INTERCHAINED LLC × Claude Fable 5
