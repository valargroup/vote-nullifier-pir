# pir-client

Private Information Retrieval client for [vote-nullifier-pir](https://github.com/valargroup/vote-nullifier-pir). Fetches circuit-ready Merkle non-membership proofs for Zcash nullifiers without revealing which nullifier is being queried to the server.

Used by Zcash wallets integrating shielded voting: before building a delegation ZKP, a wallet must prove its notes' nullifiers are absent from the on-chain nullifier set. The set has tens of millions of entries and can't be downloaded in full; `pir-client` performs a two-tier YPIR query that returns only a short authentication path.

## Usage

```rust
use pir_client::{PirClientBlocking, ImtProofData};

let client = PirClientBlocking::connect("https://pir1.example.com")?;
let proof: ImtProofData = client.fetch_proof(my_nullifier)?;
assert!(proof.verify(my_nullifier));
```

Async equivalent:

```rust
use pir_client::PirClient;

let client = PirClient::connect("https://pir1.example.com").await?;
let proofs = client.fetch_proofs(&[nf1, nf2, nf3]).await?;
```

The returned `ImtProofData { root, nf_bounds, leaf_pos, path: [Fp; 29] }` is then fed as a witness into the Zcash-voting delegation ZKP.

## Security

- The client always sends the Tier 2 query even after a Tier 1 failure, to prevent a malicious server from distinguishing queries via timing.
- Verify each proof locally with `proof.verify(nullifier)` before trusting the returned root.

## License

Dual-licensed under MIT or Apache-2.0. See [LICENSE-MIT](../../LICENSE-MIT) and [LICENSE-APACHE](../../LICENSE-APACHE).
