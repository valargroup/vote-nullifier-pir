# Private Merkle-Path Retrieval via PIR

**Version:** 0.8 — 9+6+10 Tier Layout
**Date:** 2026-03-31

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
  - [Architecture: 9 + 6 + 10](#architecture-9--6--10)
  - [Tier 0: Plaintext (Depths 0–9)](#tier-0-plaintext-depths-09)
    - [Payload](#payload)
    - [Client procedure](#client-procedure)
    - [Caching](#caching)
  - [Tier 1: PIR Query 1 (Depths 9–15)](#tier-1-pir-query-1-depths-915)
    - [Database layout](#database-layout)
    - [YPIR row padding](#ypir-row-padding)
    - [Database size](#database-size)
    - [Client procedure](#client-procedure-1)
  - [Tier 2: PIR Query 2 (Depths 15–25)](#tier-2-pir-query-2-depths-1525)
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
    - [Tier 0 layout (49,120 bytes)](#tier-0-layout-49120-bytes)
    - [Tier 1 row layout (4,096 bytes)](#tier-1-row-layout-4096-bytes)
    - [Tier 2 row layout (98,304 bytes)](#tier-2-row-layout-98304-bytes)
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
251-bit range check. The tree includes **33 sentinel nullifiers** at
positions `k × 2²⁴⁹` for k = 0, 1, …, 32 (plus an additional sentinel at
`p − 1` to close the tail of the field). These partition the Pallas field
(~2²⁵⁴) into segments where every punctured-range span fits within the
250-bit range-check bound. If the resulting sorted nullifier count is even,
a padding nullifier at value `2` is inserted so that `build_punctured_ranges`
can group them into complete K=2 triples.

---

## Problem Statement

A server holds a Merkle tree over **N ≈ 25.5 million leaves** (≤ 2²⁵), each
committing to a punctured range between sorted nullifiers. A client wants to
privately retrieve the Merkle authentication path for a given key inside a
punctured range — the 25 sibling hashes needed to verify the leaf against
the root — without revealing which key it is querying.

We use Poseidon as the hash function because authentication paths must be
verified inside a ZKP. The client computes **~2,108 Poseidon hashes** during
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
sub-second server processing. Our data regime (~3 GB database, 4–98 KB
records) falls squarely into the "large record" setting from Section 4.6 of
the paper.

| Parameter | Value |
| --------- | ----- |
| Row payload (Tier 1) | 4,096 bytes |
| Row payload (Tier 2) | 98,304 bytes |
| Tier 1 YPIR rows | 2,048 (padded from 512) |
| Tier 2 YPIR rows | 32,768 |

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

## Architecture: 9 + 6 + 10

The 25-layer tree is split into three tiers:

```
Depth 0  ──────────────  root
  │
  │   TIER 0: Plaintext (9 layers)
  │   Depths 0–9
  │
Depth 9  ──────────────  512 subtree roots
  │
  │   TIER 1: PIR Query 1 (6 layers)
  │   Depths 9–15
  │
Depth 15 ──────────────  32,768 subtree roots
  │
  │   TIER 2: PIR Query 2 (10 layers)
  │   Depths 15–25
  │
Depth 25 ──────────────  leaves (up to 33,554,432)
```

Authentication path coverage:

| Tier | Siblings provided | Depths |
| ---- | ----------------- | ------ |
| Tier 0 (plaintext) | 9 | 1–9 |
| Tier 1 (PIR query) | 6 | 10–15 |
| Tier 2 (PIR query) | 10 | 16–25 |
| **Total** | **25** | **1–25** |

The circuit expects 29 sibling hashes. The remaining 4 levels (depths 26–29)
are padded with pre-computed empty subtree hashes.

At each tier the client must learn:
- The sibling hashes along the path to the queried key
- The index to query at the next tier

For the leaf tier (Tier 2), the client also needs the full leaf data and its
sibling's data to compute the sibling hash locally.

---

## Tier 0: Plaintext (Depths 0–9)

### Payload

The server publishes a single binary blob containing two sections:

**Section 1 — Internal hashes (depths 0–8):**

All internal nodes from the root down to depth 8, in breadth-first order.

Count: 2⁰ + 2¹ + ⋯ + 2⁸ = 2⁹ − 1 = 511 hashes × 32 bytes = **16,352 bytes**

**Section 2 — Subtree records at depth 9:**

Each record is an interleaved pair: 32-byte `hash` ‖ 32-byte `min_key`.

| Field | Size | Purpose |
| ----- | ---- | ------- |
| `hash` | 32 bytes | Merkle hash of the subtree rooted here |
| `min_key` | 32 bytes | Smallest `nf_lo` in this subtree (for binary search) |

Count: 2⁹ = 512 records × 64 bytes = **32,768 bytes**

**Total Tier 0 payload: 16,352 + 32,768 = 49,120 bytes (≈ 48 KB)**

### Client procedure

1. **Binary search** the 512 `min_key` values in Section 2 to find subtree
   index `S₁ ∈ [0, 511]` such that `min_key[S₁] ≤ target_key < min_key[S₁+1]`.

2. **Read 9 sibling hashes** directly from the blob:
   - Depth 9 sibling: read `hash` from Section 2 at index `S₁ XOR 1`.
   - Depths 1–8 siblings: walk the path determined by `S₁` upward through
     the BFS-indexed internal nodes in Section 1.

   Client hashing cost: **0** — all hashes are already in plaintext.

### Caching

This payload is identical for all clients and independent of the queried key.
It changes only when the tree is rebuilt (once per governance round). At 48 KB,
it can be served via CDN, cached locally, or even bundled in application source.

---

## Tier 1: PIR Query 1 (Depths 9–15)

### Database layout

| Property | Value | Derivation |
| -------- | ----- | ---------- |
| Rows | 512 | One per depth-9 subtree |
| Content per row | 64 leaf records only | No internal nodes |

Each row contains only the 64 leaf records for the subtree. Internal nodes
are **not** stored; the client rebuilds the 6-level subtree locally from the
leaf hashes (~62 Poseidon hashes).

**Leaf records** (relative depth 6, absolute depth 15):

These are roots of Tier 2 subtrees. Each record is: 32-byte `hash` ‖ 32-byte
`min_key`.

Leaf count: 2⁶ = 64
Leaf storage: 64 × 64 = **4,096 bytes**

**Row total: 4,096 bytes**

### YPIR row padding

YPIR's SimplePIR requires a minimum of 2,048 rows (`poly_len`). Since
Tier 1 has only 512 logical rows, the YPIR database is padded with 1,536
zero-filled rows to reach the 2,048 minimum. The client must use its
logical row index (0–511) when issuing the PIR query; the server maps this
into the padded database.

### Database size

| Metric | Derivation | Result |
| ------ | ---------- | ------ |
| Logical | 512 × 4,096 | ≈ **2.0 MB** |
| YPIR (padded) | 2,048 × 4,096 | ≈ **8.0 MB** |

### Client procedure

1. Issue PIR query for **row S₁** (the subtree index from Tier 0).

2. **Binary search** the 64 `min_key` values to find sub-subtree index
   `S₂ ∈ [0, 63]`. Records are interleaved `(hash, min_key)`, so the search
   steps by stride 64 and reads `min_key` at byte offset `i × 64 + 32`.

3. **Rebuild the 6-level subtree** and extract 6 sibling hashes:
   - Read the 64 leaf `hash` values (already pre-computed; no hashing needed).
   - Build 5 internal levels bottom-up: 32 + 16 + 8 + 4 + 2 = 62
     Poseidon hashes.
   - Walk the path determined by `S₂`, collecting the sibling at each level.
   - Total: **~62 Poseidon hashes** (~0.7 ms on mobile).

---

## Tier 2: PIR Query 2 (Depths 15–25)

### Database layout

| Property | Value | Derivation |
| -------- | ----- | ---------- |
| Rows | 32,768 | One per depth-15 subtree |
| Content per row | 1,024 leaf records only | No internal nodes |

Each row contains only the 1,024 leaf records for the subtree. Internal nodes
are **not** stored; the client rebuilds the 10-level subtree locally from the
leaf data (~2,046 Poseidon hashes).

**Leaf records** (absolute depth 25 — the actual tree leaves):

Each record is: 32-byte `nf_lo` ‖ 32-byte `nf_mid` ‖ 32-byte `nf_hi`. No
separate hash field; the leaf hash is computed as
`Poseidon3(nf_lo, nf_mid, nf_hi)`.

Leaf count: 2¹⁰ = 1,024
Leaf storage: 1,024 × 96 = **98,304 bytes**

**Row total: 98,304 bytes**

### Empty-leaf padding

Partially-filled rows pad remaining entries with all-zero fields:
`nf_lo = 0, nf_mid = 0, nf_hi = 0`. The empty leaf hash
`Poseidon3(0, 0, 0)` is used as the empty subtree leaf.

### Database size

| Metric | Derivation | Result |
| ------ | ---------- | ------ |
| Raw | 32,768 × 98,304 | ≈ **3.00 GB** |

### Client procedure

1. Compute the Tier 2 row index: `S₁ × 64 + S₂` (the absolute depth-15
   subtree index).

2. Issue PIR query for this row.

3. **Binary search** the 1,024 leaf `nf_lo` values to find the target leaf.
   Records are at stride 96 and the search reads `nf_lo` at byte offset
   `i × 96`. Verify the value falls strictly inside the punctured
   range: `nf_lo < value < nf_hi` and `value ≠ nf_mid`.

4. **Rebuild the 10-level subtree** and extract 10 sibling hashes:
   - Hash all 1,024 leaf records: `Poseidon3(nf_lo, nf_mid, nf_hi)` for
     populated leaves, `Poseidon3(0, 0, 0)` for empty padding (1,024 hashes).
   - Build 9 internal levels bottom-up: 512 + 256 + 128 + 64 + 32 + 16 + 8 +
     4 + 2 = 1,022 Poseidon hashes.
   - Walk the path determined by the target leaf position, collecting the
     sibling at each level.
   - Total: **~2,046 Poseidon hashes** (~25 ms on mobile).

---

## Storage Summary

### Data stored across all tiers

| Data | Location | Size | Derivation |
| ---- | -------- | ---- | ---------- |
| Depths 0–8 internal hashes | Tier 0 | 16,352 B | (2⁹ − 1) × 32 |
| Depth-9 hashes + keys | Tier 0 | 32,768 B | 2⁹ × 64 |
| Depth-15 hashes + keys | Tier 1 rows | 2,097,152 B | 512 × 64 × 64 |
| Depth-25 leaves (nf_lo + nf_mid + nf_hi) | Tier 2 rows | 3,221,225,472 B | 32,768 × 1,024 × 96 |
| **Total** | | **≈ 3.01 GB** | |

### Server storage

| Database | Raw size | Notes |
| -------- | -------- | ----- |
| Tier 0 (plaintext) | 48 KB | Cacheable, public |
| Tier 1 (PIR) | 8.0 MB | Padded from 2 MB to 2,048 rows for YPIR |
| Tier 2 (PIR) | 3.00 GB | Binding constraint for scheme selection |
| **Total (served)** | **≈ 3.01 GB** | |

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
| Tier 0 payload | Server → Client | 48 KB |
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
| Tier 0 | Over 512 keys | 0 | 9 (read from plaintext) |
| Tier 1 | Over 64 keys | ~62 (subtree rebuild) | 6 (extracted during rebuild) |
| Tier 2 | Over 1,024 keys | ~2,046 (subtree rebuild) | 10 (extracted during rebuild) |
| **Total** | | **~2,108** | **25** |

Tier 0 serves pre-computed internal nodes, so the client reads siblings
directly. Tier 1 and Tier 2 rows store only leaf data (no internal nodes);
the client rebuilds the subtree locally for each. Tier 1: 62 `Poseidon`
hashes from pre-computed leaf hashes. Tier 2: 1,024 `Poseidon3` leaf hashes
+ 1,022 `Poseidon` internal hashes. Total: **~2,108 Poseidon calls**
(~25 ms on mobile). The ZKP circuit verifies the full 29-hash
authentication path (25 PIR siblings + 4 empty-hash padding) against the
public root.

The increase in client-side hashing relative to earlier 11+7+7 layouts is the
tradeoff for having far fewer Tier 2 rows (32,768 vs 262,144), which reduces
server-side PIR computation and the number of rows the YPIR scheme must handle.

---

## Row Serialization

### Tier 0 layout (49,120 bytes)

```
Bytes 0–16,351:        internal_nodes           511 × 32 B = 16,352 B
                       (BFS: 1 at depth 0, 2 at depth 1, ..., 256 at depth 8)
Bytes 16,352–49,119:   subtree_records[0..511]   512 × 64 B = 32,768 B
                       (each: 32-byte hash ‖ 32-byte min_key)
```

**Indexing:**

- Internal node at depth `d`, index `i`: byte offset = `((2^d − 1) + i) × 32`
- Subtree record at index `s`: byte offset = `16,352 + s × 64`
  - `hash` at `+0`, `min_key` at `+32`

### Tier 1 row layout (4,096 bytes)

Leaf records only — no internal nodes. The client rebuilds the subtree locally.

```
Bytes 0–4,095:         leaf_records[0..63]       64 × 64 B = 4,096 B
                       (each: 32-byte hash ‖ 32-byte min_key)
```

**Indexing:**

- Leaf record byte offset: `p × 64` for p ∈ [0, 64)
  - `hash` at `+0`, `min_key` at `+32`

### Tier 2 row layout (98,304 bytes)

Leaf records only — no internal nodes. The client rebuilds the subtree locally.

```
Bytes 0–98,303:        leaf_records[0..1023]     1,024 × 96 B = 98,304 B
                       (each: 32-byte nf_lo ‖ 32-byte nf_mid ‖ 32-byte nf_hi)
```

**Indexing:**

- Leaf record byte offset: `i × 96` for i ∈ [0, 1024)
  - `nf_lo` at `+0`, `nf_mid` at `+32`, `nf_hi` at `+64`
- Empty leaf records use all-zero fields.

---

## Security Properties

- **Key privacy:** The server learns nothing about which key the client
  queries. Tier 0 is identical for all clients. Tier 1 and Tier 2 queries
  are protected by PIR.
- **Sorted-tree leakage:** Key boundaries (the `min_key` values in Tier 0) are
  public. This reveals the distribution of keys across 512 depth-9
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
   boundary, Tier 1 utilisation is 4,096 / 4,096 = 100% and Tier 2
   utilisation is 98,304 / 131,072 = 75%.

4. **Client computation tradeoff:** The 9+6+10 split moves more work to the
   client (~2,108 Poseidon hashes vs ~380 in the earlier 11+7+7 layout) in
   exchange for 8× fewer Tier 2 rows (32,768 vs 262,144), reducing
   server-side PIR cost and YPIR matrix dimensions.

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
- [ZIP Draft](https://github.com/zcash/zips/blob/main/zips/draft-valargroup-nullifier-pir.md) —
  Canonical specification for the 9+6+10 tier layout.
- [imt-tree crate](../imt-tree/README.md) — Indexed Merkle Tree with sentinel
  nullifiers and circuit integration.
- Authoritative constants: [`pir/types/src/lib.rs`](../pir/types/src/lib.rs).
