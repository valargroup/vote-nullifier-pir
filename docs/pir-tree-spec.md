# Private Merkle-Path Retrieval via PIR

**Version:** 0.7 — Leaf-Only PIR Rows
**Date:** 2026-03-28

How a client privately retrieves a 25-hash Merkle authentication path from a
sorted nullifier tree using two PIR queries — without revealing which key
it is looking up.

**Contents**

- [Private Merkle-Path Retrieval via PIR](#private-merkle-path-retrieval-via-pir)
  - [Background](#background)
    - [Punctured-range leaves (K=2)](#punctured-range-leaves-k2)
    - [Sentinel invariant](#sentinel-invariant)
  - [Problem Statement](#problem-statement)
    - [Design target](#design-target)
  - [PIR Scheme: YPIR (SimplePIR)](#pir-scheme-ypir-simplepir)
  - [Constants and Sizes](#constants-and-sizes)
    - [Raw tree size](#raw-tree-size)
  - [Architecture: 11 + 7 + 7](#architecture-11--7--7)
  - [Tier 0: Plaintext (Depths 0–11)](#tier-0-plaintext-depths-011)
    - [Payload](#payload)
    - [Client procedure](#client-procedure)
    - [Caching](#caching)
  - [Tier 1: PIR Query 1 (Depths 11–18)](#tier-1-pir-query-1-depths-1118)
    - [Database layout](#database-layout)
    - [Database size](#database-size)
    - [Client procedure](#client-procedure-1)
  - [Tier 2: PIR Query 2 (Depths 18–25)](#tier-2-pir-query-2-depths-1825)
    - [Database layout](#database-layout-1)
    - [Empty-leaf padding](#empty-leaf-padding)
    - [Database size](#database-size-1)
    - [Client procedure](#client-procedure-2)
  - [Storage Summary](#storage-summary)
    - [Data stored across all tiers](#data-stored-across-all-tiers)
    - [Server storage](#server-storage)
  - [Bandwidth Summary](#bandwidth-summary)
  - [Client Computation Summary](#client-computation-summary)
  - [Row Serialization](#row-serialization)
    - [Tier 0 layout (196,576 bytes)](#tier-0-layout-196576-bytes)
    - [Tier 1 row layout (8,192 bytes)](#tier-1-row-layout-8192-bytes)
    - [Tier 2 row layout (12,288 bytes)](#tier-2-row-layout-12288-bytes)
  - [Security Properties](#security-properties)
  - [Open Questions](#open-questions)
  - [References](#references)

---

## Background

This system is part of the [Zcash Shielded Voting](https://github.com/zcash/zips/pull/1198)
protocol. To cast a shielded vote, a client must prove that its note has
**not** been spent — i.e., the note's nullifier does not appear in the on-chain
nullifier set. This is a nullifier **non-membership** proof.

The server maintains an Indexed Merkle Tree (see [`imt-tree/`](../imt-tree/))
over ~51 million Zcash Orchard nullifiers. Each leaf commits to a
**punctured range** — two adjacent gaps joined by excluding the nullifier
between them: `leaf = Poseidon3(nf_lo, nf_mid, nf_hi)`. To prove
non-membership, the client shows its nullifier falls inside one of these
punctured ranges (strictly between `nf_lo` and `nf_hi`, and not equal to
`nf_mid`).

The proof is verified inside a zero-knowledge circuit (the delegation circuit),
which requires a 25-hash Merkle authentication path from leaf to root (padded
to 29 levels for circuit compatibility).
Downloading the entire tree (~3 GB) to find one path is impractical. Instead,
the client uses **Private Information Retrieval (PIR)** to fetch exactly the
path it needs, without the server learning which nullifier is being queried.

### Punctured-range leaves (K=2)

Instead of one leaf per gap between adjacent nullifiers, each leaf covers
**two adjacent gaps** by storing three sorted nullifier boundaries
`[nf_lo, nf_mid, nf_hi]`. The leaf commitment is `Poseidon3(nf_lo, nf_mid, nf_hi)`.

This halves the number of leaves (~25.5M instead of ~51M), reducing tree
depth from 26 to 25. The circuit cost is essentially unchanged because the
extra Poseidon permutation in the leaf commitment (2 instead of 1) is offset
by fewer levels, and a single inequality check (`value ≠ nf_mid`) is trivially
cheap.

### Sentinel invariant

The circuit verifies gap widths using a range check. With K=2, the outer
span `nf_hi − nf_lo` can be up to twice a single gap width, requiring a
251-bit range check. The tree includes **17+ sentinel nullifiers** at
positions `k × 2²⁵⁰` for k = 0, 1, …, 16 (plus an extra sentinel at
`2²⁴⁹` if needed to ensure an odd nullifier count). These partition the
Pallas field (~2²⁵⁴) into segments where every punctured-range span fits
within the bound. The export process injects these sentinels before building
ranges and the tree.

---

## Problem Statement

A server holds a Merkle tree over **N ≈ 25.5 million leaves** (≤ 2²⁵), each
committing to a punctured range between sorted nullifiers. A client wants to
privately retrieve the Merkle authentication path for a given key inside a
punctured range — the 25 sibling hashes needed to verify the leaf against
the root — without revealing which key it is querying.

We use Poseidon as the hash function because authentication paths must be
verified inside a ZKP. The client computes **~380 Poseidon hashes** during
the PIR phase to rebuild Tier 1 and Tier 2 subtrees locally (the ZKP
circuit handles the full 29-level authentication path).

### Design target

Use **2 sequential PIR queries** plus a small plaintext payload to retrieve a
full 25-hash authentication path. No hash-map or ORAM overhead.

---

## PIR Scheme: YPIR (SimplePIR)

We use the SimplePIR mode of [YPIR](https://github.com/menonsamir/ypir)
(Menon & Wu, [ePrint 2024/270](https://eprint.iacr.org/2024/270.pdf),
USENIX Security 2024).

**Why YPIR+SP?** Classic SimplePIR is fast but requires a large database hint
that the client must download once per session. YPIR eliminates this hint via
silent preprocessing while retaining SimplePIR's low per-query bandwidth and
sub-second server processing. Our data regime (~3 GB database, 8–12 KB
records) falls squarely into the "large record" setting from Section 4.6 of
the paper.

| Parameter | Value |
| --------- | ----- |
| Tier 1 server processing | ~0.4 s per query (AVX-512) |
| Tier 2 server processing | ~0.9 s per query (AVX-512) |
| Row payload (Tier 1) | 8,192 bytes |
| Row payload (Tier 2) | 12,288 bytes |

See [`docs/params.md`](params.md) for full YPIR lattice parameter derivation.

---

## Constants and Sizes

| Symbol | Value | Description |
| ------ | ----- | ----------- |
| K | 32 bytes | Key size (Pallas field element) |
| V | 32 bytes | Value size (Pallas field element) |
| H | 32 bytes | Hash output size (Poseidon) |
| L | 96 bytes | Leaf record: 3 × 32-byte field elements (nf_lo, nf_mid, nf_hi) |
| D | 25 | PIR tree depth (root at 0, leaves at 25) |
| D_circuit | 29 | Circuit tree depth (padded from D with 4 empty hash levels) |

A **leaf** is a 96-byte record: `nf_lo ‖ nf_mid ‖ nf_hi`. The leaf hash is
`Poseidon3(nf_lo, nf_mid, nf_hi)` and is not stored separately.

An **internal node** is a 32-byte hash: `Poseidon(left_child, right_child)`.

### Raw tree size

| Component | Count | Size each | Total |
| --------- | ----- | --------- | ----- |
| Leaves | 2²⁵ = 33,554,432 | 96 bytes | 3.00 GB |
| Internal nodes | 2²⁵ − 1 = 33,554,431 | 32 bytes | 1.00 GB |
| **Total** | | | **≈ 4.00 GB** |

---

## Architecture: 11 + 7 + 7

The 25-layer tree is split into three tiers:

```
Depth 0  ──────────────  root
  │
  │   TIER 0: Plaintext (11 layers)
  │   Depths 0–11
  │
Depth 11 ──────────────  2,048 subtree roots
  │
  │   TIER 1: PIR Query 1 (7 layers)
  │   Depths 11–18
  │
Depth 18 ──────────────  262,144 subtree roots
  │
  │   TIER 2: PIR Query 2 (7 layers)
  │   Depths 18–25
  │
Depth 25 ──────────────  leaves (up to 33,554,432)
```

Authentication path coverage:

| Tier | Siblings provided | Depths |
| ---- | ----------------- | ------ |
| Tier 0 (plaintext) | 11 | 1–11 |
| Tier 1 (PIR query) | 7 | 12–18 |
| Tier 2 (PIR query) | 7 | 19–25 |
| **Total** | **25** | **1–25** |

The circuit expects 29 sibling hashes. The remaining 4 levels (depths 26–29)
are padded with pre-computed empty subtree hashes.

At each tier the client must learn:
- The sibling hashes along the path to the queried key
- The index to query at the next tier

For the leaf tier (Tier 2), the client also needs the full leaf data and its
sibling's data to compute the sibling hash locally.

---

## Tier 0: Plaintext (Depths 0–11)

### Payload

The server publishes a single binary blob containing two sections:

**Section 1 — Internal hashes (depths 0–10):**

All internal nodes from the root down to depth 10, in breadth-first order.

Count: 2⁰ + 2¹ + ⋯ + 2¹⁰ = 2¹¹ − 1 = 2,047 hashes × 32 bytes = **65,504 bytes**

**Section 2 — Subtree records at depth 11:**

Each record is an interleaved pair: 32-byte `hash` ‖ 32-byte `min_key`.

| Field | Size | Purpose |
| ----- | ---- | ------- |
| `hash` | 32 bytes | Merkle hash of the subtree rooted here |
| `min_key` | 32 bytes | Smallest `nf_lo` in this subtree (for binary search) |

Count: 2¹¹ = 2,048 records × 64 bytes = **131,072 bytes**

**Total Tier 0 payload: 65,504 + 131,072 = 196,576 bytes (≈ 192 KB)**

### Client procedure

1. **Binary search** the 2,048 `min_key` values in Section 2 to find subtree
   index `S₁ ∈ [0, 2047]` such that `min_key[S₁] ≤ target_key < min_key[S₁+1]`.

2. **Read 11 sibling hashes** directly from the blob:
   - Depth 11 sibling: read `hash` from Section 2 at index `S₁ XOR 1`.
   - Depths 1–10 siblings: walk the path determined by `S₁` upward through
     the BFS-indexed internal nodes in Section 1.

   Client hashing cost: **0** — all hashes are already in plaintext.

### Caching

This payload is identical for all clients and independent of the queried key.
It changes only when the tree is rebuilt (once per governance round). At 192 KB,
it can be served via CDN, cached locally, or even bundled in application source.

---

## Tier 1: PIR Query 1 (Depths 11–18)

### Database layout

| Property | Value | Derivation |
| -------- | ----- | ---------- |
| Rows | 2,048 | One per depth-11 subtree |
| Content per row | 128 leaf records only | No internal nodes |

Each row contains only the 128 leaf records for the subtree. Internal nodes
are **not** stored; the client rebuilds the 7-level subtree locally from the
leaf hashes (~126 Poseidon hashes).

**Leaf records** (relative depth 7, absolute depth 18):

These are roots of Tier 2 subtrees. Each record is: 32-byte `hash` ‖ 32-byte
`min_key`.

Leaf count: 2⁷ = 128
Leaf storage: 128 × 64 = **8,192 bytes**

**Row total: 8,192 bytes**

### Database size

| Metric | Derivation | Result |
| ------ | ---------- | ------ |
| Raw | 2,048 × 8,192 | ≈ **16.0 MB** |

### Client procedure

1. Issue PIR query for **row S₁** (the subtree index from Tier 0).

2. **Binary search** the 128 `min_key` values to find sub-subtree index
   `S₂ ∈ [0, 127]`. Records are interleaved `(hash, min_key)`, so the search
   steps by stride 64 and reads `min_key` at byte offset `i × 64 + 32`.

3. **Rebuild the 7-level subtree** and extract 7 sibling hashes:
   - Read the 128 leaf `hash` values (already pre-computed; no hashing needed).
   - Build 6 internal levels bottom-up: 64 + 32 + 16 + 8 + 4 + 2 = 126
     Poseidon hashes.
   - Walk the path determined by `S₂`, collecting the sibling at each level.
   - Total: **~126 Poseidon hashes** (~1.5 ms on mobile).

---

## Tier 2: PIR Query 2 (Depths 18–25)

### Database layout

| Property | Value | Derivation |
| -------- | ----- | ---------- |
| Rows | 262,144 | One per depth-18 subtree |
| Content per row | 128 leaf records only | No internal nodes |

Each row contains only the 128 leaf records for the subtree. Internal nodes
are **not** stored; the client rebuilds the 7-level subtree locally from the
leaf data (~254 Poseidon hashes).

**Leaf records** (absolute depth 25 — the actual tree leaves):

Each record is: 32-byte `nf_lo` ‖ 32-byte `nf_mid` ‖ 32-byte `nf_hi`. No
separate hash field; the leaf hash is computed as
`Poseidon3(nf_lo, nf_mid, nf_hi)`.

Leaf count: 2⁷ = 128
Leaf storage: 128 × 96 = **12,288 bytes**

**Row total: 12,288 bytes**

### Empty-leaf padding

Partially-filled rows pad remaining entries with all-zero fields:
`nf_lo = 0, nf_mid = 0, nf_hi = 0`. The empty leaf hash
`Poseidon3(0, 0, 0)` is used as the empty subtree leaf.

### Database size

| Metric | Derivation | Result |
| ------ | ---------- | ------ |
| Raw | 262,144 × 12,288 | ≈ **3.00 GB** |

### Client procedure

1. Compute the Tier 2 row index: `S₁ × 128 + S₂` (the absolute depth-18
   subtree index).

2. Issue PIR query for this row.

3. **Binary search** the 128 leaf `nf_lo` values to find the target leaf.
   Records are at stride 96 and the search reads `nf_lo` at byte offset
   `i × 96`. Verify the value falls strictly inside the punctured
   range: `nf_lo < value < nf_hi` and `value ≠ nf_mid`.

4. **Rebuild the 7-level subtree** and extract 7 sibling hashes:
   - Hash all 128 leaf records: `Poseidon3(nf_lo, nf_mid, nf_hi)` for
     populated leaves, `Poseidon3(0, 0, 0)` for empty padding (128 hashes).
   - Build 6 internal levels bottom-up: 64 + 32 + 16 + 8 + 4 + 2 = 126
     Poseidon hashes.
   - Walk the path determined by the target leaf position, collecting the
     sibling at each level.
   - Total: **~254 Poseidon hashes** (~3 ms on mobile).

---

## Storage Summary

### Data stored across all tiers

| Data | Location | Size | Derivation |
| ---- | -------- | ---- | ---------- |
| Depths 0–10 internal hashes | Tier 0 | 65,504 B | (2¹¹ − 1) × 32 |
| Depth-11 hashes + keys | Tier 0 | 131,072 B | 2¹¹ × 64 |
| Depth-18 hashes + keys | Tier 1 rows | 16,777,216 B | 2,048 × 128 × 64 |
| Depth-25 leaves (nf_lo + nf_mid + nf_hi) | Tier 2 rows | 3,221,225,472 B | 262,144 × 128 × 96 |
| **Total** | | **≈ 3.02 GB** | |

### Server storage

| Database | Raw size | Notes |
| -------- | -------- | ----- |
| Tier 0 (plaintext) | 192 KB | Cacheable, public |
| Tier 1 (PIR) | 16.0 MB | Small enough for any PIR scheme |
| Tier 2 (PIR) | 3.00 GB | Binding constraint for scheme selection |
| **Total (raw)** | **≈ 3.02 GB** | **50% smaller than v0.4 (6.02 GB)** |

Whether the server stores raw or padded rows depends on the PIR scheme. YPIR
operates on raw data and handles alignment internally via `FilePtIter` packing.

---

## Bandwidth Summary

Bandwidth is scheme-dependent. With YPIR+SP, the client downloads a
per-database hint once per session, then each query involves an encrypted
request and response. The dominant cost is YPIR ciphertext overhead, not the
plaintext row payload.

| Component | Direction | Size |
| --------- | --------- | ---- |
| Tier 0 payload | Server → Client | 192 KB |
| Tier 1 hint (one-time) | Server → Client | scheme-dependent |
| Tier 2 hint (one-time) | Server → Client | scheme-dependent |
| PIR Query 1 (round trip) | Both | scheme-dependent |
| PIR Query 2 (round trip) | Both | scheme-dependent |

Tier 0 can be cached across sessions since it only changes when the tree is
rebuilt.

---

## Client Computation Summary

| Step | Binary search | Hashes computed | Sibling hashes extracted |
| ---- | ------------- | --------------- | ----------------------- |
| Tier 0 | Over 2,048 keys | 0 | 11 (read from plaintext) |
| Tier 1 | Over 128 keys | ~126 (subtree rebuild) | 7 (extracted during rebuild) |
| Tier 2 | Over 128 keys | ~254 (subtree rebuild) | 7 (extracted during rebuild) |
| **Total** | | **~380** | **25** |

Tier 0 serves pre-computed internal nodes, so the client reads siblings
directly. Tier 1 and Tier 2 rows store only leaf data (no internal nodes);
the client rebuilds the 7-level subtree locally for each. Tier 1: 126
`Poseidon` hashes from pre-computed leaf hashes. Tier 2: 128 `Poseidon3`
leaf hashes + 126 `Poseidon` internal hashes. Total: **~380 Poseidon calls**
(~4.5 ms on mobile). The ZKP circuit verifies the full 29-hash
authentication path (25 PIR siblings + 4 empty-hash padding) against the
public root.

---

## Row Serialization

### Tier 0 layout (196,576 bytes)

```
Bytes 0–65,503:        internal_nodes           2,047 × 32 B = 65,504 B
                       (BFS: 1 at depth 0, 2 at depth 1, ..., 1024 at depth 10)
Bytes 65,504–196,575:  subtree_records[0..2047]  2,048 × 64 B = 131,072 B
                       (each: 32-byte hash ‖ 32-byte min_key)
```

**Indexing:**

- Internal node at depth `d`, index `i`: byte offset = `((2^d − 1) + i) × 32`
- Subtree record at index `s`: byte offset = `65,504 + s × 64`
  - `hash` at `+0`, `min_key` at `+32`

### Tier 1 row layout (8,192 bytes)

Leaf records only — no internal nodes. The client rebuilds the subtree locally.

```
Bytes 0–8,191:         leaf_records[0..127]      128 × 64 B = 8,192 B
                       (each: 32-byte hash ‖ 32-byte min_key)
```

**Indexing:**

- Leaf record byte offset: `p × 64` for p ∈ [0, 128)
  - `hash` at `+0`, `min_key` at `+32`

### Tier 2 row layout (12,288 bytes)

Leaf records only — no internal nodes. The client rebuilds the subtree locally.

```
Bytes 0–12,287:        leaf_records[0..127]      128 × 96 B = 12,288 B
                       (each: 32-byte nf_lo ‖ 32-byte nf_mid ‖ 32-byte nf_hi)
```

**Indexing:**

- Leaf record byte offset: `i × 96` for i ∈ [0, 128)
  - `nf_lo` at `+0`, `nf_mid` at `+32`, `nf_hi` at `+64`
- Empty leaf records use all-zero fields.

---

## Security Properties

- **Key privacy:** The server learns nothing about which key the client
  queries. Tier 0 is identical for all clients. Tier 1 and Tier 2 queries
  are protected by PIR.
- **Sorted-tree leakage:** Key boundaries (the `min_key` values in Tier 0) are
  public. This reveals the distribution of keys across 2,048 depth-11
  subtrees, but not which subtree any specific client queries.
- **No hash-map overhead:** The sorted tree enables binary search within each
  tier's plaintext data, eliminating the need for oblivious hash maps or
  cuckoo hashing.

---

## Open Questions

1. **Tree updates:** When leaves change, Tier 2 rows and all ancestor nodes
   are affected. Tier 1 rows change if any descendant leaf changes. Tier 0
   always changes. Incremental update cost depends on the PIR scheme's
   preprocessing model.

2. **Query sequentiality:** The two PIR queries are inherently sequential —
   Query 2's row index depends on Query 1's result. Pipelining is not possible
   without speculation (e.g., querying multiple candidate Tier 2 rows).

3. **Row utilisation:** If the PIR scheme pads rows to a power-of-two
   boundary, Tier 1 utilisation is 8,192 / 8,192 = 100% and Tier 2
   utilisation is 12,288 / 16,384 = 75%.

---

## References

- S. J. Menon and D. J. Wu.
  [YPIR: High-Throughput Single-Server PIR with Silent Preprocessing](https://eprint.iacr.org/2024/270.pdf).
  USENIX Security 2024.
- YPIR implementation: [github.com/menonsamir/ypir](https://github.com/menonsamir/ypir)
  (branch `artifact`).
- [PIR Parameter Selection](params.md) — YPIR lattice parameter derivation for
  this system.
- [Zcash ZIP Specification (PR)](https://github.com/zcash/zips/pull/1198) —
  Shielded voting protocol.
- [imt-tree crate](../imt-tree/README.md) — Indexed Merkle Tree with sentinel
  nullifiers and circuit integration.
