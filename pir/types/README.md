# pir-types

Shared wire types and tier-layout constants for [vote-nullifier-pir](https://github.com/valargroup/vote-nullifier-pir) — a YPIR-based Private Information Retrieval system that returns non-membership proofs for a ~51M-entry Zcash nullifier set without revealing which nullifier was queried.

This crate contains:

- Tier-layout constants (`PIR_DEPTH`, `TIER0_LAYERS`, `TIER1_LAYERS`, `TIER2_LAYERS`, row widths) shared between `pir-client`, `pir-server`, and `pir-export`.
- Wire types serialized over HTTP: `YpirScenario`, `RootInfo`, `HealthInfo`.
- Query serialization helper `serialize_ypir_query`.

Enable the `reader` feature to get tier-data parsers (`tier0::Tier0Data`, `tier1::Tier1Row`, `tier2::Tier2Row`) and Fp serialization helpers (`fp_utils`). The default feature set is lightweight and only pulls in `serde`.

## Usage

Pure library; consumed by `pir-client` and `pir-server`. Not typically used directly from application code.

## License

Dual-licensed under MIT or Apache-2.0. See [LICENSE-MIT](../../LICENSE-MIT) and [LICENSE-APACHE](../../LICENSE-APACHE).
