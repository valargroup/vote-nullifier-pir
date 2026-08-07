# pir-client

Private Information Retrieval client for [vote-nullifier-pir](https://github.com/valargroup/vote-nullifier-pir). Fetches circuit-ready Merkle non-membership proofs for Zcash Ironwood nullifiers without revealing which nullifier is being queried to the server.

Used by Zcash wallets integrating shielded voting: before building a delegation ZKP, a wallet must prove its notes' nullifiers are absent from the on-chain nullifier set. `pir-client` avoids downloading the full set by searching the public Tier 0 index and performing one YPIR query for the selected Tier 1 leaf row.

## Usage

```rust
use std::sync::Arc;

use pir_client::{ImtProofData, PirClientBlocking, PirLayout, Transport};

let transport: Arc<dyn Transport> = Arc::new(my_http_transport);
let expected_layout: PirLayout = resolved_voting_config.pir_layout.into();
let client = PirClientBlocking::with_transport(
    "https://pir1.example.com",
    expected_layout,
    transport,
)?;
let proof: ImtProofData = client.fetch_proof(my_nullifier)?;
assert!(proof.verify(my_nullifier));
```

Async equivalent:

```rust
use std::sync::Arc;

use pir_client::{PirClient, PirLayout, Transport};

let transport: Arc<dyn Transport> = Arc::new(my_http_transport);
let expected_layout: PirLayout = resolved_voting_config.pir_layout.into();
let client =
    PirClient::with_transport("https://pir1.example.com", expected_layout, transport).await?;
let proofs = client.fetch_proofs(&[nf1, nf2, nf3]).await?;
```

The returned `ImtProofData { root, nf_bounds, leaf_pos, path: [Fp; 29] }` is then fed as a witness into the Zcash-voting delegation ZKP.

## Security

- The client rejects servers that don't report the expected Ironwood dataset version.
- Client construction requires the layout from the resolved dynamic voting config.
- The client requires an exact config-to-`/root` layout match before parsing
  Tier 0 or creating a usable client. Missing `pir_layout` metadata fails closed.
  Any valid two-tier split that meets YPIR and circuit bounds is accepted;
  `COMPILED_PIR_LAYOUT` is not a connect gate.
- The client validates the depth/split geometry, circuit-depth bound, YPIR
  minima, Tier 1 row count, and Tier 1 row width against `/params/tier1`
  before any private query.
- Verify each proof locally with `proof.verify(nullifier)` before trusting the returned root.

## License

Dual-licensed under MIT or Apache-2.0. See [LICENSE-MIT](../../LICENSE-MIT) and [LICENSE-APACHE](../../LICENSE-APACHE).
