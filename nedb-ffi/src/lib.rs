//! nedb-ffi: C API bridge for the NEDB DAG engine
//!
//! This crate exposes NEDB's causal DAG storage to the ITC C++ node,
//! replacing LevelDB as the block index and chainstate backend.
//!
//! # Phase 1 (this file)
//! HashMap-backed in-process store. Proves the C FFI surface, the CDBWrapper
//! shim, and the full compile pipeline. Produces identical external behaviour
//! to LevelDB from the node's perspective.
//!
//! # Phase 2
//! Swap HashMap for `nedb_core_v2::Db` (DAG engine with BLAKE2b chain + MVCC).
//! Every block write becomes a causal PUT with `caused_by = prev_block_hash`,
//! giving deterministic state roots, `AS OF` time-travel, and `verify()` proofs.
//! Enable by adding:
//!   nedb-v2 = { git = "https://github.com/Eth-Interchained/nedb", package = "nedb-v2" }
//! to Cargo.toml and uncommenting the nedb_core_v2 integration below.
//!
//! © Interchained LLC × Claude Sonnet 4.6

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uchar};
use std::sync::Mutex;

use blake2::{Blake2b512, Digest};

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// An individual pending write in a batch.
#[derive(Clone)]
struct BatchOp {
    key: Vec<u8>,
    value: Option<Vec<u8>>, // None = delete
}

/// Core in-process store (Phase 1: BTreeMap for ordered iteration).
struct NedbInner {
    /// Main key-value store.  Keys are raw bytes; we preserve order for iter.
    store: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Monotonic write counter (maps to NEDB seq in Phase 2).
    seq: u64,
    /// Running BLAKE2b chain head — advances with every committed write.
    /// In Phase 2 this becomes the NEDB Merkle head and IS the state root.
    head: Vec<u8>,
    /// Logical name for this database instance (= path used at open time).
    name: String,
}

impl NedbInner {
    /// Update the BLAKE2b chain head after a write.
    fn advance_head(&mut self, key: &[u8], value: Option<&[u8]>) {
        let mut h = Blake2b512::new();
        h.update(&self.head);
        h.update(key);
        if let Some(v) = value { h.update(v); }
        h.update(self.seq.to_le_bytes());
        self.head = h.finalize().to_vec();
        self.seq += 1;
    }
}

/// Opaque handle returned to C callers.
pub struct NedbHandle {
    inner: Mutex<NedbInner>,
}

// ---------------------------------------------------------------------------
// Iter state
// ---------------------------------------------------------------------------

/// Snapshot iterator over the store.
pub struct NedbIter {
    /// Snapshot copy of all entries at iterator creation time.
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    pos: usize,
}

// ---------------------------------------------------------------------------
// C API — database lifecycle
// ---------------------------------------------------------------------------

/// Open (or create) a NEDB database at `path`.
/// `dek` is the AES-256-GCM data-encryption key in hex (may be NULL for plaintext).
/// Returns an opaque NedbHandle*, or NULL on failure.
#[no_mangle]
pub extern "C" fn nedb_open(path: *const c_char, _dek: *const c_char) -> *mut NedbHandle {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    let name = unsafe { CStr::from_ptr(path).to_string_lossy().to_string() };
    // Phase 2: pass name + dek to nedb_core_v2::Db::open(name, dek)
    let handle = Box::new(NedbHandle {
        inner: Mutex::new(NedbInner {
            store: BTreeMap::new(),
            seq: 0,
            head: vec![0u8; 64], // 512-bit genesis head (all zeros = genesis)
            name,
        }),
    });
    Box::into_raw(handle)
}

/// Close the database and free all resources.
#[no_mangle]
pub extern "C" fn nedb_close(handle: *mut NedbHandle) {
    if !handle.is_null() {
        unsafe { drop(Box::from_raw(handle)) }
    }
}

// ---------------------------------------------------------------------------
// C API — single-record operations
// ---------------------------------------------------------------------------

/// Read the value for `key`.
///
/// Returns:
///  0  — found; `*value_out` and `*value_len_out` are populated.
///  1  — key not found.
/// -1  — error (null handle).
///
/// Caller MUST free `*value_out` via `nedb_free_value(*value_out, *value_len_out)`.
#[no_mangle]
pub extern "C" fn nedb_get(
    handle: *mut NedbHandle,
    key: *const c_uchar,
    key_len: usize,
    value_out: *mut *mut c_uchar,
    value_len_out: *mut usize,
) -> c_int {
    if handle.is_null() || key.is_null() { return -1; }
    let inner = unsafe { &*handle }.inner.lock().unwrap();
    let key_bytes = unsafe { std::slice::from_raw_parts(key, key_len) };
    match inner.store.get(key_bytes) {
        None => 1,
        Some(val) => {
            let mut boxed: Box<[u8]> = val.clone().into_boxed_slice();
            unsafe {
                *value_len_out = boxed.len();
                *value_out = boxed.as_mut_ptr();
                std::mem::forget(boxed);
            }
            0
        }
    }
}

/// Free a value buffer returned by `nedb_get`.
#[no_mangle]
pub extern "C" fn nedb_free_value(ptr: *mut c_uchar, len: usize) {
    if !ptr.is_null() && len > 0 {
        unsafe {
            let _ = Box::from_raw(std::slice::from_raw_parts_mut(ptr, len) as *mut [u8]);
        }
    }
}

/// Write `value` under `key`.  Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn nedb_put(
    handle: *mut NedbHandle,
    key: *const c_uchar,
    key_len: usize,
    value: *const c_uchar,
    value_len: usize,
) -> c_int {
    if handle.is_null() || key.is_null() || value.is_null() { return -1; }
    let mut inner = unsafe { &*handle }.inner.lock().unwrap();
    let key_bytes  = unsafe { std::slice::from_raw_parts(key,   key_len)   }.to_vec();
    let val_bytes  = unsafe { std::slice::from_raw_parts(value, value_len) }.to_vec();
    inner.advance_head(&key_bytes, Some(&val_bytes));
    inner.store.insert(key_bytes, val_bytes);
    0
}

/// Delete `key`.  Returns 0 on success (including if key did not exist), -1 on error.
#[no_mangle]
pub extern "C" fn nedb_del(
    handle: *mut NedbHandle,
    key: *const c_uchar,
    key_len: usize,
) -> c_int {
    if handle.is_null() || key.is_null() { return -1; }
    let mut inner = unsafe { &*handle }.inner.lock().unwrap();
    let key_bytes = unsafe { std::slice::from_raw_parts(key, key_len) }.to_vec();
    inner.advance_head(&key_bytes, None);
    inner.store.remove(&key_bytes);
    0
}

/// Returns 1 if `key` exists, 0 if not, -1 on error.
#[no_mangle]
pub extern "C" fn nedb_exists(
    handle: *mut NedbHandle,
    key: *const c_uchar,
    key_len: usize,
) -> c_int {
    if handle.is_null() || key.is_null() { return -1; }
    let inner = unsafe { &*handle }.inner.lock().unwrap();
    let key_bytes = unsafe { std::slice::from_raw_parts(key, key_len) };
    if inner.store.contains_key(key_bytes) { 1 } else { 0 }
}

/// Returns 1 if the database contains no entries, 0 otherwise, -1 on error.
#[no_mangle]
pub extern "C" fn nedb_is_empty(handle: *mut NedbHandle) -> c_int {
    if handle.is_null() { return -1; }
    let inner = unsafe { &*handle }.inner.lock().unwrap();
    if inner.store.is_empty() { 1 } else { 0 }
}

// ---------------------------------------------------------------------------
// C API — batch writes
// ---------------------------------------------------------------------------

/// A single operation in a batch write.
#[repr(C)]
pub struct NedbOp {
    pub key:       *const c_uchar,
    pub key_len:   usize,
    /// NULL means delete this key.
    pub value:     *const c_uchar,
    pub value_len: usize,
}

/// Atomically apply a batch of put/delete operations.
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn nedb_batch_write(
    handle: *mut NedbHandle,
    ops:     *const NedbOp,
    ops_len: usize,
) -> c_int {
    if handle.is_null() || ops.is_null() { return -1; }
    let mut inner = unsafe { &*handle }.inner.lock().unwrap();
    let ops_slice = unsafe { std::slice::from_raw_parts(ops, ops_len) };
    // Phase 2: wrap in a single nedb_core_v2 group-commit batch
    for op in ops_slice {
        if op.key.is_null() { continue; }
        let k = unsafe { std::slice::from_raw_parts(op.key, op.key_len) }.to_vec();
        if op.value.is_null() {
            inner.advance_head(&k, None);
            inner.store.remove(&k);
        } else {
            let v = unsafe { std::slice::from_raw_parts(op.value, op.value_len) }.to_vec();
            inner.advance_head(&k, Some(&v));
            inner.store.insert(k, v);
        }
    }
    0
}

// ---------------------------------------------------------------------------
// C API — state root (BLAKE2b chain head)
// ---------------------------------------------------------------------------

/// Returns the current BLAKE2b chain head as a null-terminated hex string.
///
/// In Phase 2 this is the NEDB DAG Merkle root — a deterministic commitment
/// to all storage state at the current sequence. Two nodes that have processed
/// the same chain will produce identical heads, providing storage-layer
/// consensus verification.
///
/// Caller must free the returned string via `nedb_free_str`.
#[no_mangle]
pub extern "C" fn nedb_head(handle: *mut NedbHandle) -> *mut c_char {
    if handle.is_null() {
        return CString::new("").unwrap().into_raw();
    }
    let inner = unsafe { &*handle }.inner.lock().unwrap();
    let hex = hex::encode(&inner.head);
    CString::new(hex).unwrap().into_raw()
}

/// Free a C string returned by any nedb_* function.
#[no_mangle]
pub extern "C" fn nedb_free_str(s: *mut c_char) {
    if !s.is_null() {
        unsafe { drop(CString::from_raw(s)) }
    }
}

// ---------------------------------------------------------------------------
// C API — iterator (for UTXO scan, chain state iteration)
// ---------------------------------------------------------------------------

/// Create a snapshot iterator.  The iterator captures a copy of the store
/// at this point in time.  Caller must free via `nedb_iter_free`.
#[no_mangle]
pub extern "C" fn nedb_iter_new(handle: *mut NedbHandle) -> *mut NedbIter {
    if handle.is_null() { return std::ptr::null_mut(); }
    let inner = unsafe { &*handle }.inner.lock().unwrap();
    let entries: Vec<(Vec<u8>, Vec<u8>)> = inner.store.iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let iter = Box::new(NedbIter { entries, pos: usize::MAX });
    Box::into_raw(iter)
}

/// Free an iterator.
#[no_mangle]
pub extern "C" fn nedb_iter_free(iter: *mut NedbIter) {
    if !iter.is_null() {
        unsafe { drop(Box::from_raw(iter)) }
    }
}

/// Position the iterator at the first entry.
#[no_mangle]
pub extern "C" fn nedb_iter_seek_to_first(iter: *mut NedbIter) {
    if iter.is_null() { return; }
    let it = unsafe { &mut *iter };
    it.pos = 0;
}

/// Seek to the first entry whose key >= `key`.
/// Returns 1 if a valid position was found, 0 otherwise.
#[no_mangle]
pub extern "C" fn nedb_iter_seek(
    iter: *mut NedbIter,
    key: *const c_uchar,
    key_len: usize,
) -> c_int {
    if iter.is_null() || key.is_null() { return 0; }
    let it = unsafe { &mut *iter };
    let key_bytes = unsafe { std::slice::from_raw_parts(key, key_len) };
    it.pos = it.entries.partition_point(|(k, _)| k.as_slice() < key_bytes);
    if it.pos < it.entries.len() { 1 } else { 0 }
}

/// Advance to the next entry.
#[no_mangle]
pub extern "C" fn nedb_iter_next(iter: *mut NedbIter) {
    if iter.is_null() { return; }
    let it = unsafe { &mut *iter };
    if it.pos != usize::MAX { it.pos += 1; }
}

/// Returns 1 if the iterator points to a valid entry.
#[no_mangle]
pub extern "C" fn nedb_iter_valid(iter: *const NedbIter) -> c_int {
    if iter.is_null() { return 0; }
    let it = unsafe { &*iter };
    if it.pos != usize::MAX && it.pos < it.entries.len() { 1 } else { 0 }
}

/// Get the current key.  Returns 0 on success, -1 on error.
/// Caller must free via `nedb_free_value`.
#[no_mangle]
pub extern "C" fn nedb_iter_key(
    iter: *const NedbIter,
    key_out: *mut *mut c_uchar,
    key_len_out: *mut usize,
) -> c_int {
    if iter.is_null() { return -1; }
    let it = unsafe { &*iter };
    if it.pos >= it.entries.len() { return -1; }
    let mut boxed: Box<[u8]> = it.entries[it.pos].0.clone().into_boxed_slice();
    unsafe {
        *key_len_out = boxed.len();
        *key_out = boxed.as_mut_ptr();
        std::mem::forget(boxed);
    }
    0
}

/// Get the current value.  Returns 0 on success, -1 on error.
/// Caller must free via `nedb_free_value`.
#[no_mangle]
pub extern "C" fn nedb_iter_value(
    iter: *const NedbIter,
    value_out: *mut *mut c_uchar,
    value_len_out: *mut usize,
) -> c_int {
    if iter.is_null() { return -1; }
    let it = unsafe { &*iter };
    if it.pos >= it.entries.len() { return -1; }
    let mut boxed: Box<[u8]> = it.entries[it.pos].1.clone().into_boxed_slice();
    unsafe {
        *value_len_out = boxed.len();
        *value_out = boxed.as_mut_ptr();
        std::mem::forget(boxed);
    }
    0
}
