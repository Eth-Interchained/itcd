# ITC / NEDB Session Handoff
*June 20, 2026 — for continuity across sessions*

---

## What Was Built Today

### The Core Achievement
`interchainedd` — a Bitcoin-derived full node with NEDB replacing LevelDB at the `CDBWrapper` seam (`src/dbwrapper_nedb.cpp`). Running on ITC mainnet. Mac syncing ~183 blocks/second during IBD.

### Platform Status
| Platform | Status | Notes |
|----------|--------|-------|
| macOS x86_64 (iMac Intel) | ✅ Syncing mainnet | Codemagic build, `xattr -cr` to run |
| Windows x86_64 | ✅ Green binary | GitHub Actions artifact, 80MB |
| Linux portable | ✅ Green binary | GitHub Actions artifact |

---

## Architecture

### Storage Layer
- `nedb-ffi/` — Rust staticlib (`libnedb_ffi.a`) linked into `interchainedd`
- Phase 2 default: `nedb-core-v2::Db` — content-addressed DAG, BLAKE2b-named objects, AES-256-GCM optional
- Object layout: `objects/{hash[..2]}/{hash[2..]}` — immutable, atomic writes via `.tmp` + `fs::rename`
- ID index: `indexes/{coll}/id/{2-hex-shard}/{hex_key}` → file content = object hash
- MANIFEST: Merkle root over all collection heads, flushed every 5s

### Key FFI Functions
```c
nedb_open(path, dek)        // opens database, returns NedbHandle*
nedb_scan(handle, cb, ctx)  // sequential file walk, fires callback per entry with progress
nedb_get(handle, key, ...)  // single direct lookup
nedb_scan_callback signature: (key, klen, val, vlen, progress, total, ctx)
```

### Startup Path (warm boot)
```
ReadTipHash()        → single NEDB get → tip block hash
ReadTipChainWork()   → single NEDB get → tip nChainWork
LoadBlockIndexFromTip(tip, 2016)  → 2016 direct NEDB lookups backwards via hashPrev
  └── lazy loading: when BuildSkip() needs ancestor outside window → fetch from NEDB on demand
```

**First run** (no tip hash stored): falls back to `nedb_scan` full directory walk — one time only.

**Every subsequent restart**: warm boot → seconds, not hours.

### Key DB Keys (txdb.cpp)
```cpp
static const char DB_BLOCK_INDEX    = 'b';  // block header entries
static const char DB_CHAIN_WORK_TIP = 'W';  // persisted tip nChainWork
static const char DB_TIP_HASH       = 'T';  // persisted tip block hash
```

Both `W` and `T` are written in `AddToBlockIndex` (validation.cpp ~line 3302) whenever `pindexBestHeader` advances.

---

## Files Changed This Session

| File | What Changed |
|------|-------------|
| `nedb-ffi/nedb.h` | Added `nedb_scan` declaration + `NedbScanFn` typedef |
| `nedb-ffi/src/lib.rs` | `nedb_scan` sequential file walk; `NedbHandle` got `path: PathBuf` field |
| `nedb-ffi/Cargo.toml` | Added `rayon` as explicit dependency (phase2 feature) |
| `src/dbwrapper.h` | Added `protected GetHandle()` accessor |
| `src/txdb.h` | Added `WriteTipHash`, `ReadTipHash`, `LoadBlockIndexFromTip`, `WriteTipChainWork`, `ReadTipChainWork`; added `arith_uint256.h` include |
| `src/txdb.cpp` | Implemented all of the above; added `DB_CHAIN_WORK_TIP` + `DB_TIP_HASH` keys; `LoadBlockIndexFromTip` walks backwards from tip via direct NEDB reads |
| `src/validation.cpp` | Warm boot fast path in `LoadBlockIndex`; writes tip hash+chainwork in `AddToBlockIndex`; lazy ancestor loading in chain work loop before `BuildSkip()` |
| `codemagic.yaml` | Developer ID signing step (needs `CM_CERTIFICATE` + `CM_CERTIFICATE_PASSWORD` from `itcd_signing` group); `itcd_signing` env group added to both arm64 and x86_64 workflows |
| `.github/workflows/*.yml` | All three workflows changed to `workflow_dispatch` only — no auto-trigger on push |
| `docs/foundation.md` | Technical sovereignty document grounded in verified source code |

---

## Pending / Next Session

### Signing (blocked on cert)
The code signing pipeline is wired in `codemagic.yaml` but the `.p12` keeps failing import.

**Root cause**: `developer_id.key` (openssl-generated) and the Developer ID Application cert are in different keychains and not paired.

**Clean path** (do this first thing):
1. Revoke current Developer ID Application cert at developer.apple.com
2. Generate new CSR using **Keychain Access** (not openssl) — Certificate Assistant → Request from CA → Saved to disk
3. Submit to Apple → download `.cer` → double-click → auto-pairs in login keychain
4. Export from login keychain as `.p12` → password `ITC2026`
5. `base64 -i ~/Desktop/Certificates.p12 | pbcopy`
6. Update `CM_CERTIFICATE` in Codemagic `itcd_signing` group

### Warm Boot Verification
After first successful warm boot, verify chain work by comparing tip's cumulative work against peer-reported tip. Currently self-healing via peer comparison at connection time — acceptable for single-pool ITC network.

### NEDB Engine: `par_list()` 
The `nedb_scan` currently walks index files directly (bypassing `db.list()`). The right long-term fix is adding a native `par_list()` to `nedb-core-v2` using rayon. This would be faster on SSD and keep the FFI layer clean.

### Startup Cache Not Needed
Rejected by Mark. The tip-anchored warm boot IS the solution. Startup cache was redundant.

### Progress Logging
`LoadBlockIndex: X / Y (pct%)` fires every 500 entries during warm boot. During full scan it fires every 10k. Both are in the `LoadIndexCallback` in `src/txdb.cpp`.

---

## Network Details
- ITC mainnet seed: `seed.interchained.org:17101`
- One mining pool, 30-60s block time, ~830k blocks total as of June 2026
- Mark's iMac: Intel x86_64, spinning disk, ~183 blocks/sec IBD throughput
- Node binary: `interchainedd -addnode=seed.interchained.org:17101`
- Clean shutdown: `./interchained-cli stop` or `Ctrl+C` — **never `Ctrl+Z`** (SIGTSTP won't flush NEDB WAL)

---

## Important Grounding Facts (from verified source)

From `nedb-core-v2 v2.2.10` (`rust/nedb-v2/src/store.rs`):
- Every write: `.tmp` + `fs::rename` — atomic, no partial writes
- Every read: BLAKE2b verify — tampered object detected on first access
- MANIFEST: Merkle root of all collection heads — proves entire store state with one hash
- No AOF replay — comment in `lib.rs:12`: *"Instant cold start: no AOF replay"*

The `[nedbd] warm start — seq=N head=XXXX` line in debug.log comes from inside `nedb_open()` in Rust, after the NEDB store is opened and the sequence/head are read from MANIFEST.

---

## Mark's Priorities (stated)
1. Ship a daemon that boots in seconds ← **done architecturally, needs first clean resync**
2. Windows binary ← ✅ green
3. Signed Mac binary ← blocked on cert pairing
4. NEDB on crates.io ← deferred until API stabilizes
5. Never trigger full chain scan again after first run ← ✅ tip-anchored warm boot

---

*© INTERCHAINED LLC × Claude Sonnet 4.6*
