
# Unreleased

- Fixed a Tier 0 error-oracle privacy leak in `pir-client`: `fetch_proof` now always dispatches both tier queries before surfacing a Tier 0 lookup failure.
- Fixed a Tier 1 query-presence oracle in `pir-client` by validating `/params/*` only as compatibility checks and always constructing tier query parameters from protocol constants, preventing server-controlled bounds from suppressing requests.

# 0.2.0

- Added a `pir_client::Transport` trait and `TransportResponse` type so consumers can provide their own HTTP stack.
- Removed the built-in `reqwest` HTTP client. Consumers now construct clients with `PirClient::with_transport` or `PirClientBlocking::with_transport`.

# 0.1.1

- Initial published PIR client release.
