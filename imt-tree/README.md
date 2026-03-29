# imt-tree

Indexed Merkle Tree (IMT) for nullifier non-membership proofs in [Zcash shielded voting](https://github.com/zcash/zips/pull/1198). This crate provides Poseidon hashing, punctured-range tree building, and exclusion proof generation that feeds into the delegation circuit.

## Architecture Overview

### Scale and Capacity

Zcash mainnet currently has ~51M Orchard nullifiers (documented upper bound: 64M). The tree is sized for up to **256M nullifiers** (future-proof):

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Current nullifiers | ~51M | Mainnet as of Feb 2026 |
| Design capacity | 256M | Future-proof headroom |
| Leaves per tree | `(n + 1) / 2` punctured ranges for `n + sentinels` nullifiers | K=2: each leaf covers two gaps |
| `TREE_DEPTH` | **29** | `2^29 = 536M` leaf slots, comfortably above 256M |

### Punctured-Range Exclusion Model (K=2)

Rather than an inclusion tree, this is an **exclusion (non-membership) tree**. The key insight: to prove a note is **unspent**, we must show its nullifier does NOT appear on-chain.

Given sorted on-chain nullifiers `n0, n1, n2, ...`, each leaf commits to a **punctured range** — three consecutive nullifier boundaries `[nf_lo, nf_mid, nf_hi]`:

```
Sorted nullifiers:  n0=0, n1, n2, n3, n4, ...

Punctured ranges (leaves):
  [n0, n1, n2]   -- covers gaps (n0,n1) and (n1,n2), excluding n1
  [n2, n3, n4]   -- covers gaps (n2,n3) and (n3,n4), excluding n3
  ...
```

Each leaf commitment is `Poseidon3(nf_lo, nf_mid, nf_hi)`. To prove a value `x` is NOT a nullifier, show it falls strictly inside one of these punctured ranges: `nf_lo < x < nf_hi` and `x != nf_mid`.

This halves the number of leaves compared to single-gap ranges, reducing the PIR tree depth from 26 to 25 and cutting tier-2 storage by ~33%.

### Tree Structure and Empty-Slot Optimization

The tree exploits precomputed empty subtree hashes:

```
empty[0]  = Poseidon3(0, 0, 0)                  -- empty leaf
empty[1]  = Poseidon(empty[0], empty[0])         -- empty 2-leaf subtree
empty[2]  = Poseidon(empty[1], empty[1])         -- empty 4-leaf subtree
...
empty[28] = Poseidon(empty[27], empty[27])       -- empty 2^28-leaf subtree
```

During tree construction (`build_levels`), any odd-length layer is padded with the appropriate `empty[level]`, and all-empty subtrees collapse to their precomputed hash. This gives:

- **Deterministic root**: the root depends only on the nullifier set, not on tree capacity
- **O(29) proof generation**: walk pre-stored levels, falling back to `empty[level]` for out-of-bounds siblings

## PoseidonHasher Optimization

Building the tree requires hashing millions of nodes. The naive `poseidon::Hash::init().hash()` re-allocates round constants on every call (~6 KiB heap per init).

`PoseidonHasher` fixes this by computing P128Pow5T3 constants once and implementing the permutation inline:

- **R_F = 8** full rounds (4 + 4), **R_P = 56** partial rounds
- Width-3 state: `[left, right, capacity]`
- `hash(a, b)` for 2-input (internal nodes), `hash3(a, b, c)` for 3-input (leaf commitments)
- Verified equivalent to the upstream `halo2_gadgets` hasher

## Circuit Integration (Condition 13)

The delegation circuit proves 15 conditions for up to 4 notes. For each note, **condition 13** verifies IMT non-membership via four sub-checks:

### Leaf Hash (2 Poseidon permutations)

With K=2 punctured-range leaves, the leaf commitment uses `Poseidon3`:

```
leaf_hash = Poseidon3(nf_lo, nf_mid, nf_hi)
```

`Poseidon3` is `ConstantLength<3>` over a width-3 sponge (rate 2), which
requires **two** absorption blocks:

```
state = [nf_lo, nf_mid, capacity_3]    (capacity_3 = 3 × 2^64)
permute(state)
state[0] += nf_hi; state[1] += 0       (second block: one element + padding)
permute(state)
leaf_hash = state[0]
```

This is one extra permutation compared to the old `Poseidon(low, width)` leaf.

### Merkle Path (29 Poseidon permutations)

At each of the 29 levels:

1. **`q_imt_swap` gate**: Conditionally swap `(current, sibling)` based on `pos_bit`:

   ```
   left  = current + pos_bit × (sibling - current)
   right = sibling + pos_bit × (current - sibling)
   ```

2. **Poseidon hash**: `parent = Poseidon(left, right)` via `PoseidonChip`

The final computed root is constrained equal to the public `nf_imt_root`
input (in the `q_per_note` gate).

### Interval Check (`q_punctured_interval` gate + 2 range checks + 1 non-equality)

Proves `nf_lo < real_nf < nf_hi` and `real_nf ≠ nf_mid`.

**Strict lower bound** (`nf_lo < real_nf`):

```
x = real_nf - nf_lo - 1
```

Range-check `x ∈ [0, 2^251)`. If `real_nf ≤ nf_lo`, then `x` wraps to a
huge field element (`≥ p - 2^251`), failing the range check.

**Strict upper bound** (`real_nf < nf_hi`):

```
x_upper = nf_hi - real_nf - 1
```

Range-check `x_upper ∈ [0, 2^251)`. If `real_nf ≥ nf_hi`, then `x_upper`
wraps or equals `p - 1`, failing the range check.

**Non-equality** (`real_nf ≠ nf_mid`):

Prove that `(real_nf - nf_mid)` has a multiplicative inverse:

```
diff     = real_nf - nf_mid
diff_inv = inverse(diff)                 -- witness
constrain: diff × diff_inv = 1
```

If `real_nf = nf_mid` then `diff = 0`, which has no inverse, so the
constraint cannot be satisfied.

**Range-check width**: Each strict-inequality range check must accommodate
spans up to `2^251` (the maximum K=2 outer span with sentinel spacing
`2^250`). This requires **251-bit** range checks — one bit wider than the
old K=1 model's 250 bits. A tighter 250-bit bound can be recovered by
halving sentinel spacing to `2^249` (33 sentinels instead of 17).

### Root Pinning (`q_per_note` gate)

```
("imt_root = nf_imt_root", imt_root - nf_imt_root)
```

This is **NOT gated on `is_note_real`** — even padded (dummy) notes must
carry valid IMT proofs. Padded notes still get real nullifiers derived from
their zero-value notes, and the circuit verifies non-membership for all 4
slots uniformly.

### Cost Summary Per Note Slot

| Component | Custom Gates | Poseidon Calls | Range Check Limbs | Other |
|-----------|-------------|---------------|-------------------|-------|
| Leaf hash | — | 2 (Poseidon3) | — | — |
| Merkle path | 29 `q_imt_swap` | 29 | — | — |
| Interval check | 1 `q_punctured_interval` | — | 2 × 26 = 52 (for 251-bit) | 1 inverse |
| Root check | 1 `q_per_note` (shared) | — | — | — |
| **Total per note** | **31** | **31** | **52** | **1** |
| **Total (4 notes)** | **124** | **124** | **208** | **4** |

The 124 in-circuit Poseidon permutations for condition 13 are the dominant
constraint count contributor for the IMT check.

## Data Flow: Off-Chain to In-Circuit

```
                      OFF-CHAIN (imt-tree + pir crates)
                      ─────────────────────────────────
Zcash chain --> nf-server ingest --> nullifiers.bin (51M nullifiers)
                                          |
                                  prepare_nullifiers()
                                  (sort, sentinels, K=2 ranges)
                                          |
                          +───────────────+───────────────+
                          │  ~25.5M punctured ranges,     │
                          │  depth-25 PIR tree + depth-29  │
                          │  root (extended with empties)  │
                          +───────────────+───────────────+
                                          |
                             PIR fetch → ImtProofData {
                                           root, nf_bounds[3],
                                           leaf_pos, path[29]
                                         }
                                          |
                         ─────────────────|───────────────────────
                         IN-CIRCUIT       | (orchard delegation circuit)
                         ─────────────────|───────────────────────
                                          v
                      NoteSlotWitness.imt_{nf_bounds, leaf_pos, path}
                                          |
                      +───────────────────+───────────────────+
                      │  Poseidon3(nf_lo, nf_mid, nf_hi)      │  leaf hash
                      │  29× swap + Poseidon --> root          │  Merkle path
                      │  strict interval + non-equality check  │  interval proof
                      │  q_per_note: root = nf_imt_root        │  root pin
                      +───────────────────────────────────────+
                                          |
                                    proof verified
```

The `ImtProofData` struct is the seam between the off-chain and
in-circuit worlds — identical fields on both sides:

- Off-circuit: `imt_tree::ImtProofData` (in this crate)
- In-circuit: `orchard::delegation::imt::ImtProofData` (in the `orchard` crate)

The `ImtProvider` trait abstracts over the proof source (real PIR client vs.
test provider) so the delegation builder works uniformly for production and
testing.

## Sentinel Nullifiers: Circuit-Compatibility Constraint

The delegation circuit proves that a value falls inside a punctured range using a **range check** over the span `nf_hi - nf_lo`. With K=2 each leaf's outer span can cover two adjacent gaps, so the maximum span between boundary nullifiers is up to twice the width of a single gap. The Pallas field is ~2^254, so without additional structure, spans could far exceed the range-check capacity.

Solution: **17 sentinel nullifiers** at `k * 2^250` for `k = 0..=16`, plus `p - 1` to close the tail:

```
Sentinel placement on the Pallas field [0, p):

  |--2^250--|--2^250--|--2^250--| ... |--2^250--|--remainder--|
  0       2^250    2*2^250           15*2^250  16*2^250    p-1

Between consecutive sentinels:
  - single gap width ≤ 2^250 - 2
  - K=2 outer span   ≤ 2^251 (two gaps joined)
```

Since `p ~ 16.something × 2^250`, 17 sentinels cover the entire field. Adding real nullifiers only **splits** existing gaps into smaller ones, so the invariant holds permanently once established.

`prepare_nullifiers()` in `pir-export` merges these sentinels with real nullifiers, sorts, deduplicates, and pads to odd count before building punctured ranges. The tree crate itself is sentinel-agnostic — it takes pre-sorted nullifiers and builds ranges — but `verify_punctured_range_spans()` validates that no outer span exceeds `2^251`.

A tighter bound (`≤ 2^250`) can be achieved by doubling sentinel density (spacing `2^249`), if the circuit's range check requires it.

## Key Files

| File | Role |
|------|------|
| `src/tree/mod.rs` | Core tree logic: `build_levels`, `build_punctured_ranges`, `find_punctured_range_for_value`, `precompute_empty_hashes` |
| `src/hasher.rs` | Optimised `PoseidonHasher` (precomputed round constants, `hash` and `hash3`) |
| `src/proof.rs` | `ImtProofData` with out-of-circuit `verify()` |
