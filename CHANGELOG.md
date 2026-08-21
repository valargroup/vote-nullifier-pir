# Unreleased

- Log successful `nf-server` readiness locally instead of creating an
  Info-level Sentry issue. Snapshot-stale and recovery events now share an
  explicit routing tag, and recovery reports the last nonzero gap.

# v0.0.43

- Default PIR and IMT builds to the Zakura voting crypto backend while retaining
  an explicitly selectable and tested upstream backend.
- Route default PIR and voting-config reads through the GitHub-primary
  `voting.valargroup.dev` gateway with its Cloudflare fallback.

# imt-tree 0.3.0, pir-types 0.4.0, and pir-client 0.5.0

- Add mutually exclusive Zakura and upstream crypto backends, with Zakura as
  the default and minimal VCT-only dependency sets in both modes.

# v0.0.42

- Released the exact `v0.0.42-rc.1` server implementation as stable without
  implementation changes.

# pir-types 0.3.0 and pir-client 0.4.0

- Released the exact `pir-types 0.3.0-rc.6` and `pir-client 0.4.0-rc.7`
  implementations as stable without API or wire-format changes.

# v0.0.42-rc.1

- Advertise the process-configured YPIR degree at both
  `/root.pir_layout.poly_len` and `/params/tier1.poly_len` so clients can
  require one layout identity across signed configuration and the server
  handshake.

# pir-types 0.3.0-rc.6 and pir-client 0.4.0-rc.7

- `PirLayout` now includes `poly_len` (2048 or 4096; missing snapshot metadata
  deserializes as `DEFAULT_YPIR_POLY_LEN`). Serving `/root` overwrites it with
  the process-configured YPIR degree so wallets authenticate degree as part of
  the layout handshake.
- `PirClient::with_transport` / `PirClientBlocking::with_transport` take a single
  `expected_layout` and fail closed when `/root.pir_layout` or
  `GET /params/tier1` disagrees (including `poly_len`).

# v0.0.42-alpha.2

- Default YPIR lattice dimension to 4096 and advertise `poly_len` on
  `/params/tier1` (responses that omit the field stay legacy degree 2048).
- Depend on released `valar-ypir` 0.2.0 from crates.io.
- Limit PIR query request bodies to 2 MiB.
- Disable Sentry tracing in `nf-server` so proxy-added client identity headers,
  request timing, and PIR request cardinality are not exported. Process error
  reporting and explicit startup/watchdog messages remain enabled.

# pir-types 0.3.0-rc.5 and pir-client 0.4.0-rc.5

- Advertise and negotiate YPIR polynomial degree via `YpirScenario.poly_len`
  (server default 4096; responses that omit the field deserialize as legacy
  2048).
- Depend on `valar-ypir` 0.2.0 and construct YPIR clients with
  `YPIRSPConfig::for_poly_len` for supported degrees 2048 and 4096.

# v0.0.42-alpha.1

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
