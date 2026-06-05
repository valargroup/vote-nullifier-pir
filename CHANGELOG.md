
# Unreleased

- Fixed a Tier 0 error-oracle privacy leak in `pir-client`: `fetch_proof` now always dispatches both tier queries before surfacing a Tier 0 lookup failure.
- Fixed a Tier 1 query-presence oracle in `pir-client` by validating `/params/*` only as compatibility checks and always constructing tier query parameters from protocol constants, preventing server-controlled bounds from suppressing requests.
- Fixed batched `fetch_proofs` cancellation behavior in `pir-client`: replaced fail-fast `try_join_all` with `join_all` so one note's error cannot cancel siblings before tier-2 dispatch, and added a deterministic regression test for this race.

# 0.2.0

- Added a `pir_client::Transport` trait and `TransportResponse` type so consumers can provide their own HTTP stack.
- Removed the built-in `reqwest` HTTP client. Consumers now construct clients with `PirClient::with_transport` or `PirClientBlocking::with_transport`.

# 0.1.1

- Initial published PIR client release.
