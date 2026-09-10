# Mainnet Ironwood verification fixtures

Captured on 2026-09-10. These are public Zcash blocks, not synthetic chains.

- `mainnet-3428143.bin` through `mainnet-3428150.bin`: complete serialized blocks
  returned by archival zcashd `getblock(hash, 0)` on
  `roman-zcashd-compat-mainnet`, accessed through authenticated SSH.
- `mainnet-3428143.json` and `mainnet-3428150.json`: heights, displayed block
  hashes, ordered transaction IDs, and transaction Merkle roots returned by that
  node's `getblock(hash, 1)`. The latter also records independently obtained
  Ironwood nullifier bytes and Orchard action count from `zec.rocks` `GetBlock`.
- `snapshot.json`: expected roots and all 43 nullifier occurrences obtained by
  the pre-change release binary's compact sync against `https://zec.rocks:443`,
  from activation through height 3,428,150 in a fresh directory. Nullifier hex is
  canonical field-byte order; block hashes and txids use RPC display order.

The ending hash was cross-checked using `getblockhash(3428150)` on a separate
mainnet node. Roots were independently reproduced by the new raw-block verifier
using live archival RPC. See [the validation record](../../../../docs/verify-root.md).

Do not regenerate expected values from the code under test. When replacing
fixtures, record their source and independently derive transaction IDs, pool
contents, and roots. These fixtures cover the first eight Ironwood blocks only;
they do not represent validation of every later upgrade or a current-tip snapshot.
