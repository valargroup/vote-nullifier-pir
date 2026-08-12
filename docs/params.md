# PIR Parameters

This document derives the concrete tree and YPIR parameters for the
depth-19, 12+7 Ironwood layout.

## Parameter flow

```text
pir/types
  PIR_DEPTH, TIER0_LAYERS, TIER1_LAYERS
  TIER1_ROWS, TIER1_ROW_BYTES, TIER1_ITEM_BITS
        │
        ▼
pir/server::tier1_scenario()
  YpirScenario { num_items, item_size_bits }
        │
        ├──► YPIR server: params_for_scenario_simplepir()
        │
        └──► GET /params/tier1
                 │
                 ▼
             PIR client: YPIRClient::from_db_sz(...)
```

The server and client derive the same lattice parameters from the two
scenario integers. `/root` advertises the explicit depth and tier split plus
the row count and byte width. Client construction receives the expected layout
from dynamic voting configuration and requires that config match the server
advertisement before parsing Tier 0 or issuing a query. Within the two-tier
envelope, any split that satisfies YPIR minima and circuit depth is accepted;
`COMPILED_PIR_LAYOUT` (12+7) remains the production default identity for
export and server advertise, not a connect-time gate.

## Tree constants

The constants in `pir/types/src/lib.rs` are the production default:

- `PIR_DEPTH = 19`;
- `TIER0_LAYERS = 12`;
- `TIER1_LAYERS = 7`;
- `TIER1_ROWS = 1 << 12 = 4,096`;
- `TIER1_LEAVES = 1 << 7 = 128`;
- `TIER1_LEAF_BYTES = 3 * 32 = 96`;
- `TIER1_ROW_BYTES = 128 * 96 = 12,288`;
- `TIER1_ITEM_BITS = 12,288 * 8 = 98,304`.

Alternate two-tier splits such as 11+8 and 13+6 are valid when advertised
consistently in config and `/root` and when they meet YPIR minima
(`rows >= 2048`, item bits `>= 28,672`).

The following invariants are compile-time assertions:

```text
19 == 12 + 7
4,096 >= 2,048 YPIR minimum rows
98,304 >= 28,672 YPIR minimum item bits
```

No logical or physical row padding is required.

## Plaintext dimensions

Tier 0 contains every internal node above depth 12 plus one subtree record
per Tier 1 row:

```text
internal_nodes = 2^12 - 1 = 4,095
internal_bytes = 4,095 * 32 = 131,040
record_bytes   = 4,096 * (32-byte hash + 32-byte min_key)
               = 262,144
tier0_bytes    = 393,184
```

## YPIR scenario

`pir/server::tier1_scenario()` returns:

```rust
YpirScenario {
    num_items: 4_096,
    item_size_bits: 98_304,
    poly_len: 4_096,
}
```

The scenario is served at `GET /params/tier1`. There is no second scenario or
second PIR endpoint.

## SimplePIR matrix derivation

The local YPIR implementation uses:

```text
poly_len = 4,096
p        = 2^14
bits_per_polynomial = 4,096 * 14 = 57,344
```

The database matrix is therefore:

```text
db_rows = num_items
        = 4,096

db_cols = ceil(item_size_bits / 57,344)
        = ceil(98,304 / 57,344)
        = 2
```

The row dimension exponent is:

```text
nu_1 = log2(next_power_of_two(db_rows)) - 12
     = 12 - 12
     = 0
```

SimplePIR uses `nu_2 = 1`. The database width is
`instances = db_cols = 2`.

Concrete scenario summary:

- `num_items = 4,096`;
- `item_size_bits = 98,304`;
- `db_rows = 4,096`;
- `db_cols = 2`;
- `nu_1 = 0`;
- `nu_2 = 1`;
- `instances = 2`;
- source database bytes = `4,096 * 12,288 = 50,331,648` (48 MiB).

## Fixed cryptographic constants

These values remain selected by the YPIR crate's
`params_for_scenario_simplepir` and `internal_params_for`:

- plaintext modulus `p = 16,384` (`2^14`);
- ciphertext compression width `q2_bits = 28`;
- NTT moduli `[268369921, 249561089]`;
- polynomial length `4,096`;
- Gaussian noise width `16.042421`;
- RLWE rank `n = 1`;
- `t_gsw = 3`;
- `t_conv = 4`;
- `t_exp_left = 4`;
- `t_exp_right = 2`.

The client passes `true` to `YPIRClient::from_db_sz`, selecting the
SimplePIR-backed YPIR+SP path.

## Reconstruction parameters

The client obtains:

- 12 siblings from the public Tier 0;
- 7 siblings by rebuilding the returned 128-leaf Tier 1 row;
- 10 empty-hash siblings from the fixed circuit padding.

This gives the 29-element authentication path expected by `ImtProofData`:

```text
7 + 12 + 10 = 29
```

The leaf position fits in 19 bits. The exporter enforces at most `2^19`
punctured ranges, corresponding to approximately 1.05 million K=2 nullifier
boundaries.

## Upstream references

- [YPIR: High-Throughput Single-Server PIR with Silent Preprocessing](https://eprint.iacr.org/2024/270.pdf)
- [menonsamir/ypir](https://github.com/menonsamir/ypir), artifact branch
  parameter selection in `src/params.rs`
