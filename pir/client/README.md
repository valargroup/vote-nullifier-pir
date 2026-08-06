# pir-client

Private Information Retrieval client for [vote-nullifier-pir](https://github.com/valargroup/vote-nullifier-pir). Fetches circuit-ready Merkle non-membership proofs for Zcash Ironwood nullifiers without revealing which nullifier is being queried to the server.

Used by Zcash wallets integrating shielded voting: before building a delegation ZKP, a wallet must prove its notes' nullifiers are absent from the on-chain nullifier set. `pir-client` avoids downloading the full set by searching the public Tier 0 index and performing one YPIR query for the selected Tier 1 leaf row.

## Usage

```rust
use std::sync::Arc;

use pir_client::{ImtProofData, PirClientBlocking, Transport};

let transport: Arc<dyn Transport> = Arc::new(my_http_transport);
let client = PirClientBlocking::with_transport("https://pir1.example.com", transport)?;
let proof: ImtProofData = client.fetch_proof(my_nullifier)?;
assert!(proof.verify(my_nullifier));
```

Async equivalent:

```rust
use std::sync::Arc;

use pir_client::{PirClient, Transport};

let transport: Arc<dyn Transport> = Arc::new(my_http_transport);
let client = PirClient::with_transport("https://pir1.example.com", transport).await?;
let proofs = client.fetch_proofs(&[nf1, nf2, nf3]).await?;
```

The returned `ImtProofData { root, nf_bounds, leaf_pos, path: [Fp; 29] }` is then fed as a witness into the Zcash-voting delegation ZKP.

## Security

- The client rejects servers that don't report the expected Ironwood dataset version.
- The client validates the PIR depth, Tier 1 row count, and Tier 1 row width reported by `/root` against the YPIR scenario.
- Verify each proof locally with `proof.verify(nullifier)` before trusting the returned root.

## License

Dual-licensed under MIT or Apache-2.0. See [LICENSE-MIT](../../LICENSE-MIT) and [LICENSE-APACHE](../../LICENSE-APACHE).
