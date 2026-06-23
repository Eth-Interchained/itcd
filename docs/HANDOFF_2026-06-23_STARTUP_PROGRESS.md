# itcd Startup Progress Handoff — 2026-06-23

## Context

Mark reported that the NEDB-backed daemon startup can take too long and does not show enough visual progress when it bogs down.

The provided run used:

```bash
./interchainedd -addnode=seed.interchained.org:17101 -dbcache=150 -reindex
```

Because `-reindex` is explicit, startup intentionally takes the full rebuild path instead of the Proof-of-Prefix warm-boot path. The `blocks/index` and `chainstate` NEDB directories are wiped through the existing `CDBWrapper(..., fWipe=true)` path, then rebuilt from flat block files and peer sync.

## Source-grounded finding

The visual gap was real in two places:

1. `nedb-ffi/src/lib.rs::nedb_scan()` counted id-index entries before beginning the real callback scan. On large NEDB stores this pre-count phase could take visible time while C++ showed only the earlier open/scanning messages.
2. `src/validation.cpp::LoadExternalBlockFile()` only logged the start and end of each block-file reindex. If a file was slow, the operator saw little evidence that import was still alive.

## Change

This patch is intentionally itcd-side only and does not touch the NEDB engine repo or consensus rules.

- `nedb_scan()` now emits status-only discovery callbacks with `total=0` while counting entries.
- `LoadIndexCallback()` treats those callbacks as heartbeat logs: `LoadBlockIndex: discovering NEDB entries... N seen`.
- Real block-index scan progress now includes percentage, entries/sec, and elapsed milliseconds.
- Reindex block-file import now logs periodic progress after the first block, every 1000 accepted blocks, or every 10 seconds.

## Expected operator impact

Startup still does the same real work, but it no longer looks frozen during the expensive discovery/reindex phases. Logs should show whether time is being spent discovering NEDB entries, scanning NEDB records, or importing blocks from `blk*.dat`.

## Test note

The Hyperagent sandbox used for this patch did not have `cargo`, `rustc`, or `g++` installed, so local compilation could not be completed there. `git diff --check` passed and the changes are source-reviewed for the exact callback contract and C++ logging path.
