# Private Merkle-Path Retrieval via PIR

**Version:** 0.9 — 12+7 Tier Layout
**Date:** 2026-08-06

This document specifies how a client privately retrieves a circuit-compatible
Merkle authentication path for an Ironwood nullifier non-membership proof.
The client downloads one small public index and performs one YPIR query.

## Ironwood sizing

Ironwood has approximately 31,000 nullifiers at the time of this design. The
previous depth-25 layout was sized for roughly 51 million Orchard nullifiers
and imposed a second sequential PIR query that is unnecessary at Ironwood
scale.

The PIR tree now has depth 19:

- 524,288 punctured-range leaf slots (`2^19`);
- each K=2 leaf represents two adjacent nullifier gaps;
- capacity is approximately 1.05 million nullifiers, about 34 times the
  current Ironwood set;
- the exporter rejects more than `2^19` ranges.

Crossing that limit requires changing the layout constants and regenerating
all snapshots. It does not require a circuit change as long as the PIR depth
does not exceed the circuit depth.

## Leaves and roots

Nullifiers are sorted and converted into K=2 punctured ranges:

```text
[nf_lo, nf_mid, nf_hi]
```

The leaf commitment is:

```text
Poseidon3(nf_lo, nf_mid, nf_hi)
```

A queried value is covered when it is strictly between `nf_lo` and `nf_hi`
and differs from `nf_mid`. Sentinel nullifiers at `k * 2^249` for
`k = 0..=32`, plus `p - 1`, preserve the circuit's range-check invariant and
cover the Pallas field tail.

The exporter builds a depth-19 PIR root, then extends it to the delegation
circuit's depth-29 root by repeatedly hashing the current root as the left
child with the corresponding empty subtree as the right child.

## Architecture: 12 + 7

```text
Depth 0   root
  │
  │  Tier 0: plaintext, 12 layers
  │
Depth 12  4,096 subtree roots and min_key boundaries
  │
  │  Tier 1: one PIR query, 7 layers
  │
Depth 19  up to 524,288 punctured-range leaves
```

The invariant is enforced at compile time:

```text
PIR_DEPTH == TIER0_LAYERS + TIER1_LAYERS
19 == 12 + 7
```

## Tier 0: public index

`tier0.bin` contains:

- 4,095 internal hashes from depths 0 through 11:
  `4,095 * 32 = 131,040` bytes;
- 4,096 records at depth 12, each `hash || min_key`:
  `4,096 * 64 = 262,144` bytes.

Total Tier 0 size is 393,184 bytes (about 384 KiB). Nodes are BFS ordered.
The payload is public, immutable for a snapshot, and suitable for CDN and
client caching.

The client binary-searches the 4,096 `min_key` values to select a depth-12
subtree and extracts the 12 public sibling hashes above it. The selected
subtree index is the Tier 1 PIR row index; no value derived from a first PIR
response is needed.

## Tier 1: the PIR database

Tier 1 has 4,096 rows, one for each depth-12 subtree. Each row contains 128
leaf records:

```text
record i:
  offset i * 96 +  0: nf_lo   (32-byte canonical Fp)
  offset i * 96 + 32: nf_mid  (32-byte canonical Fp)
  offset i * 96 + 64: nf_hi   (32-byte canonical Fp)
```

The dimensions are:

- leaves per row: `2^7 = 128`;
- row size: `128 * 96 = 12,288` bytes;
- item size: `12,288 * 8 = 98,304` bits;
- database size: `4,096 * 12,288 = 50,331,648` bytes (48 MiB).

Unpopulated records are zero-filled. The final logical row may be partially
populated; `num_ranges` tells the client how many records are valid so zero
padding cannot be mistaken for a range.

The shape satisfies YPIR without database padding:

- 4,096 rows is at least the 2,048-row `poly_len` minimum;
- 98,304 item bits is at least the 28,672-bit minimum item width.

For SimplePIR this gives `db_rows = 4,096`, `db_cols = 4`, `nu_1 = 1`,
`nu_2 = 1`, and `instances = 4`.

## Client procedure

For a query value `v`:

1. Download or reuse cached `tier0.bin` and `/root`.
2. Binary-search Tier 0's 4,096 `min_key` boundaries to obtain row `s`.
3. Copy the 12 Tier 0 siblings into path positions 7 through 18.
4. Send one encrypted YPIR query for Tier 1 row `s`.
5. Decode the 12,288-byte row and binary-search its valid `nf_lo` values.
6. Check `nf_lo < v < nf_hi` and `v != nf_mid`.
7. Rebuild the seven-level subtree and copy its siblings into path positions
   0 through 6. This hashes at most 128 Poseidon3 leaves and 126 internal
   Poseidon2 nodes (approximately 3 ms on the measured client).
8. Copy the ten precomputed empty hashes into positions 19 through 28.
9. Return `[nf_lo, nf_mid, nf_hi]`, the global leaf position, the 29 siblings,
   and the depth-29 root.

The resulting path has 19 real siblings and 10 generic empty-hash padding
levels. `ImtProofData::verify` reconstructs the same depth-29 root used by the
delegation circuit.

## Storage, bandwidth, and computation

- Public snapshot payload: 393,184-byte Tier 0.
- Private database: one 48 MiB Tier 1 database.
- Online retrieval: one encrypted query and one encrypted response.
- Client subtree work: seven levels instead of the previous ten-level leaf
  tier, approximately eight times fewer leaf commitments.
- Server PIR work: 48 MiB instead of the previous approximately 3 GiB leaf
  database, approximately 64 times less source data.

Encrypted query and response sizes are determined by the YPIR parameters and
are reported by the benchmark harness rather than fixed by this tree format.

## Metadata and compatibility

Dataset contract version 2 identifies this layout. `pir_root.json` records:

- the Zcash network, `nullifier_pool = "ironwood"`, and dataset version;
- the PIR-depth and circuit-depth roots;
- `num_ranges`, `pir_depth`, required `pir_layout`, and snapshot height;
- Tier 0 byte size;
- `tier1_rows` and `tier1_row_bytes`.

`GET /root` returns `pir_layout = { pir_depth, tier0_layers, tier1_layers }`
alongside the legacy top-level depth and Tier 1 shape. Client construction
requires the expected layout from dynamic voting configuration and rejects
unless config and server layouts match exactly. It also rejects inconsistent
splits, depths beyond the 29-layer circuit, layouts below YPIR minima, and
row geometry that disagrees with `/params/tier1`, all before any private
query. Clients do **not** require equality against `COMPILED_PIR_LAYOUT`.
Servers validate metadata `pir_layout` and require on-disk `tier0.bin` /
`tier1.bin` sizes to match that layout before loading the precompute cache.
Snapshots missing `pir_layout` fail to deserialize. Version-1 snapshots and
depth-25 tree checkpoints are intentionally incompatible.

## Privacy

YPIR hides the selected Tier 1 row from the server under its standard
single-server computational PIR assumptions. Query and response sizes are
independent of the selected row.

Tier 0 is public and exposes 4,096 `min_key` boundaries. This is finer
snapshot-wide partition information than the previous 512-boundary layout,
but it remains independent of any client's query. The client never sends the
binary-search result in plaintext.

The raw debug endpoint `GET /tier1/row/:idx` is not privacy-preserving and
must not be used by clients; production reverse proxies may block it.

## Unchanged components

- Delegation circuit depth remains 29.
- PIR-to-circuit padding remains generic in `PIR_DEPTH`.
- `pir/export` extends the PIR root with empty hashes.
- `imt-tree::build_levels` accepts the depth at runtime.
- The YPIR SimplePIR scheme and wire encoding are unchanged.

## Operational migration

A version-2 snapshot must be published and the fleet redeployed, staging
first. Old tier files, precompute caches, and tree checkpoints cannot be
reused. Precompute cache headers remain bound to the Tier 1 source hash, so a
snapshot change automatically causes a cache miss and rewrite.

`DATASET_VERSION` also labels `nullifiers.dataset.json`. The first version-2
publication must therefore run `publish-snapshot.yml` with
`reset_dataset=true`, which re-streams the publisher's raw nullifier dataset;
this is not merely a tier-file rebuild.

The ZIP draft at `zips/draft-valargroup-nullifier-pir.md` is maintained
outside this repository and must be updated alongside this specification.
