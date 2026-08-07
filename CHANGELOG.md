# Unreleased

- Make two-tier PIR geometry runtime-driven: clients accept any valid
  `PirLayout` that matches `/root` (and passes geometry / YPIR / circuit
  bounds) instead of requiring equality against `COMPILED_PIR_LAYOUT`.
  Production default remains 12+7; `COMPILED_PIR_LAYOUT` is only the default
  export/advertise identity.
- Require `pir_layout` on `pir_root.json` / `PirMetadata` (breaking for
  snapshots that omit it). Export writes it; server load and export
  completeness checks derive expected `tier0`/`tier1.bin` sizes from that
  layout.
- Parameterize Tier 0 / Tier 1 readers and path assembly by negotiated
  layout; covered by reconstruct + connect tests for 11+8, 12+7, and 13+6.
- Add explicit depth and tier-split metadata to `/root`, and require PIR client
  construction to match the dynamic-config layout against the server before
  any private query.
- Rename the depth-specific `root25` and `root29` fields to `pir_root` and
  `circuit_root`; legacy metadata field names remain accepted as aliases.
- Accept `-alpha.N` release tags as prereleases without updating GitHub Latest
  or mutable installer aliases.
- Always issue a Tier 1 PIR request when Tier 0 routing fails or panics,
  preventing malicious routing metadata from leaking nullifier ranks through
  request counts.
- Add held stable releases and a verified manual promotion path so coordinated
  PIR upgrades can publish tag-scoped artifacts without changing installer
  aliases early, then promote every alias without permitting a stale rollback.
  Ordinary stable releases become Latest only after those aliases are published.

# pir-types 0.3.0-rc.3 and pir-client 0.4.0-rc.3

- Replace the depth-25, two-query PIR tree with the Ironwood-sized depth-19
  12+7 layout and a single PIR round trip.
- Require dataset contract v2 and validate tier geometry before serving or
  querying snapshots.

# pir-types 0.3.0-rc.2 and pir-client 0.4.0-rc.2

- Add Zcash network identity to Ironwood ingestion, snapshot artifacts, and `/root` responses.
- Keep RC installers tag-scoped; stable tags update the latest aliases.

# pir-types 0.3.0-rc.1 and pir-client 0.4.0-rc.1

- Switched PIR ingestion, artifacts, bootstrap metadata, and clients to an Ironwood-only dataset identity. Existing raw datasets require one explicit reset.

# 0.2.0

- Added a `pir_client::Transport` trait and `TransportResponse` type so consumers can provide their own HTTP stack.
- Removed the built-in `reqwest` HTTP client. Consumers now construct clients with `PirClient::with_transport` or `PirClientBlocking::with_transport`.

# 0.1.1

- Initial published PIR client release.
