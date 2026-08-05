# Unreleased

- Add held stable releases and a verified manual promotion path so coordinated
  PIR upgrades can publish tag-scoped artifacts without changing installer
  aliases early, then promote every alias without permitting a stale rollback.

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
