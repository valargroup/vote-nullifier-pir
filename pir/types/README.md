# pir-types

Shared wire types and tier-layout constants for [vote-nullifier-pir](https://github.com/valargroup/vote-nullifier-pir) — a YPIR-based Private Information Retrieval system for the Zcash Ironwood nullifier set.

This crate contains:

- Tier-layout constants (`PIR_DEPTH`, `TIER0_LAYERS`, `TIER1_LAYERS`, row widths)
  for the production default 12+7 split, plus `PirLayout` geometry helpers for
  any valid two-tier negotiation.
- Wire types serialized over HTTP: `YpirScenario`, `RootInfo`, `HealthInfo`,
  `PirMetadata`. Root and snapshot metadata include required `pir_layout` plus
  the Zcash network and Ironwood dataset identity.
- Query serialization helper `serialize_ypir_query`.

Enable the `reader` feature to get tier-data parsers (`tier0::Tier0Data`, `tier1::Tier1Row`) and Fp serialization helpers (`fp_utils`). The default `upstream` backend keeps this crate lightweight until `reader` activates the crypto dependencies. A Zakura reader must use `default-features = false, features = ["reader", "zakura"]`. The two backend features are mutually exclusive.

## Usage

Pure library; consumed by `pir-client` and `pir-server`. Not typically used directly from application code.

## License

Dual-licensed under MIT or Apache-2.0. See [LICENSE-MIT](../../LICENSE-MIT) and [LICENSE-APACHE](../../LICENSE-APACHE).
