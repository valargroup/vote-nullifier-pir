# pir-client

Private Information Retrieval client for [vote-nullifier-pir](https://github.com/valargroup/vote-nullifier-pir). Fetches circuit-ready Merkle non-membership proofs for Zcash Ironwood nullifiers without revealing which nullifier is being queried to the server.

Used by Zcash wallets integrating shielded voting: before building a delegation ZKP, a wallet must prove its notes' nullifiers are absent from the on-chain nullifier set. `pir-client` avoids downloading the full set by performing a two-tier YPIR query that returns only a short authentication path.

## Usage

```rust
use std::sync::Arc;

use pir_client::{ImtProofData, PirClientBlocking, Transport, ZcashNetwork};

let transport: Arc<dyn Transport> = Arc::new(my_http_transport);
let client = PirClientBlocking::with_transport(
    "https://pir1.example.com",
    ZcashNetwork::Test,
    transport,
)?;
let proof: ImtProofData = client.fetch_proof(my_nullifier)?;
assert!(proof.verify(my_nullifier));
```

Async equivalent:

```rust
use std::sync::Arc;

use pir_client::{PirClient, Transport, ZcashNetwork};

let transport: Arc<dyn Transport> = Arc::new(my_http_transport);
let client = PirClient::with_transport(
    "https://pir1.example.com",
    ZcashNetwork::Test,
    transport,
).await?;
let proofs = client.fetch_proofs(&[nf1, nf2, nf3]).await?;
```

The returned `ImtProofData { root, nf_bounds, leaf_pos, path: [Fp; 29] }` is then fed as a witness into the Zcash-voting delegation ZKP.

## Security

- The client rejects servers that don't report the expected Zcash network and Ironwood dataset version.
- The client always sends the Tier 2 query even after a Tier 1 failure, to prevent a malicious server from distinguishing queries via timing.
- Verify each proof locally with `proof.verify(nullifier)` before trusting the returned root.

## License

Dual-licensed under MIT or Apache-2.0. See [LICENSE-MIT](../../LICENSE-MIT) and [LICENSE-APACHE](../../LICENSE-APACHE).
