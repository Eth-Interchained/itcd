// Copyright (c) 2012-2019 The Interchained Core developers
// Distributed under the MIT software license, see the accompanying
// file COPYING or http://www.opensource.org/licenses/mit-license.php.
//
// NEDB backend implementation of CDBWrapper / CDBBatch / CDBIterator.
// This file replaces src/dbwrapper.cpp when compiling itcd with NEDB storage.
//
// Architecture:
//   ITC C++ consensus layer (P2P, PoW, ITSL, scripts — unchanged)
//       ↕  CDBWrapper shim (this file)
//   nedb-ffi C API  (nedb-ffi/src/lib.rs — cbindgen bridge)
//       ↕  Rust FFI
//   NEDB DAG engine (Phase 2: nedb_core_v2::Db)
//       ↕  nedbd HTTP / nedbd --dag
//   NEDB Studio (NQL explorer, TRACE provenance, AS OF time-travel)
//
// Phase 1: HashMap/BTreeMap-backed in-process store (proves the seam).
// Phase 2: wire in nedb_core_v2::Db — BLAKE2b chain head, MVCC, causal DAG.
//
// © Interchained LLC × Claude Sonnet 4.6

#include <dbwrapper.h>
#include <logging.h>
#include <util/system.h>

#include <cstdio>
#include <cstring>
#include <stdexcept>
#include <string>
#include <vector>

// ---------------------------------------------------------------------------
// dbwrapper_private
// ---------------------------------------------------------------------------

namespace dbwrapper_private {

// Zero-key obfuscation vector — NEDB uses AES-256-GCM encryption instead.
// All XOR operations against this key are no-ops, preserving wire compatibility
// with code that calls GetObfuscateKey() without needing actual XOR obfuscation.
static const std::vector<unsigned char> kZeroObfuscateKey(
    CDBWrapper::OBFUSCATE_KEY_NUM_BYTES, 0);

const std::vector<unsigned char>& GetObfuscateKey(const CDBWrapper& w)
{
    // Return the wrapper's own key (always zero for NEDB backend).
    return w.obfuscate_key;
}

} // namespace dbwrapper_private

// ---------------------------------------------------------------------------
// CDBWrapper static members
// ---------------------------------------------------------------------------

const std::string  CDBWrapper::OBFUSCATE_KEY_KEY    = "\x0bobfuscate_key";
const unsigned int CDBWrapper::OBFUSCATE_KEY_NUM_BYTES = 8;

// ---------------------------------------------------------------------------
// CDBWrapper lifecycle
// ---------------------------------------------------------------------------

CDBWrapper::CDBWrapper(const fs::path& path, size_t nCacheSize,
                       bool fMemory, bool fWipe, bool obfuscate)
    : pdb(nullptr)
{
    m_name = path.string();

    if (fWipe) {
        LogPrintf("NEDB: wiping data directory %s\n", m_name);
        // Phase 2: call nedb_wipe(path) or remove the data directory.
        // Phase 1: nothing to wipe — in-memory BTreeMap starts empty.
    }

    // Obfuscation is a no-op for NEDB (encryption is at the engine level).
    obfuscate_key = std::vector<unsigned char>(OBFUSCATE_KEY_NUM_BYTES, 0);

    // Phase 2: pass the TMK (from NEDB_TMK env var) as the dek parameter.
    // Phase 1: dek is nullptr (no encryption in the HashMap backend).
    const char* dek = nullptr;
    // const char* dek = getenv("NEDB_TMK");  // uncomment for Phase 2 encryption

    LogPrintf("NEDB: opening database '%s'%s\n",
              m_name,
              fMemory ? " (in-memory)" : "");

    pdb = nedb_open(m_name.c_str(), dek);
    if (!pdb) {
        throw dbwrapper_error("NEDB: failed to open database: " + m_name);
    }

    LogPrintf("NEDB: opened database '%s'\n", m_name);
}

CDBWrapper::~CDBWrapper()
{
    nedb_close(pdb);
    pdb = nullptr;
}

std::vector<unsigned char> CDBWrapper::CreateObfuscateKey() const
{
    // NEDB backend: always returns zero vector (obfuscation is not used).
    return std::vector<unsigned char>(OBFUSCATE_KEY_NUM_BYTES, 0);
}

// ---------------------------------------------------------------------------
// CDBWrapper write path
// ---------------------------------------------------------------------------

bool CDBWrapper::WriteBatch(CDBBatch& batch, bool /*fSync*/)
{
    if (batch.m_ops.empty()) return true;

    // Build the NedbOp array from the accumulated batch entries.
    // The NedbBatchEntry vectors keep the byte data alive for this call.
    std::vector<NedbOp> ops;
    ops.reserve(batch.m_ops.size());

    for (const auto& entry : batch.m_ops) {
        NedbOp op;
        op.key     = entry.key.data();
        op.key_len = entry.key.size();
        if (entry.is_delete) {
            op.value     = nullptr;
            op.value_len = 0;
        } else {
            op.value     = entry.value.data();
            op.value_len = entry.value.size();
        }
        ops.push_back(op);
    }

    int rc = nedb_batch_write(pdb, ops.data(), ops.size());
    if (rc != 0) {
        throw dbwrapper_error("NEDB: batch write failed for database: " + m_name);
    }

    // fSync is a no-op: NEDB's group-commit Sequencer handles fsync internally.
    // Phase 2: the DAG engine syncs on every commit by design.
    return true;
}

// ---------------------------------------------------------------------------
// CDBWrapper misc
// ---------------------------------------------------------------------------

bool CDBWrapper::IsEmpty()
{
    int rc = nedb_is_empty(pdb);
    if (rc < 0) {
        throw dbwrapper_error("NEDB: is_empty failed for database: " + m_name);
    }
    return rc == 1;
}

// ---------------------------------------------------------------------------
// CDBIterator lifecycle
// ---------------------------------------------------------------------------

CDBIterator::~CDBIterator()
{
    nedb_iter_free(piter);
    piter = nullptr;
}

bool CDBIterator::Valid() const
{
    return nedb_iter_valid(piter) == 1;
}

void CDBIterator::SeekToFirst()
{
    nedb_iter_seek_to_first(piter);
}

void CDBIterator::Next()
{
    nedb_iter_next(piter);
}
