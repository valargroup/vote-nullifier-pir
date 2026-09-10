# Verify a proposed Ironwood nullifier root

`nf-server verify-root` downloads complete raw blocks, authenticates their
transaction effects against a trusted snapshot block hash, and reconstructs the
Ironwood nullifier tree. Exit status zero means its depth-29 circuit root equals
the proposed root. The command is included in every `nf-server` build; it does not
require `serve` or a separate Cargo feature.

## Trust contract and security argument

The operator must independently authenticate **all three** of the following:

- The Zcash network (`main` or `test`).
- The exact snapshot height.
- The accepted block hash at that height, in conventional RPC/explorer hex order.

Obtain that tuple from a validating node or another explicitly trusted channel.
A tuple supplied only by the party proposing the nullifier root is not an
independent anchor. Agreement between two URLs is not automatically agreement
between independent operators. The CLI cannot establish the provenance of an
argument supplied by its caller.

Given that authenticated tuple, the raw-block provider may be dishonest. Starting
at the trusted ending hash, the verifier fetches `getblock(hash, 0)` and follows
each authenticated predecessor backward through Ironwood activation, inclusively.
At each height it:

1. Parses a complete, bounded block and rejects trailing bytes.
2. Recomputes the header hash and checks it against the expected hash.
3. Checks the expected descending coinbase height and rejects an empty transaction list.
4. Recomputes every transaction ID, rejects duplicate transaction IDs, and requires
   their ordered Merkle root to equal the header's transaction commitment.
5. Extracts all Ironwood action nullifiers from the verified transaction effects.

Under the binding assumptions of the protocol hashes, removing/changing an
Ironwood action changes its transaction ID; excluding a transaction changes the
block's transaction commitment; excluding a block breaks the predecessor chain.
Duplicate transaction IDs are rejected explicitly because Bitcoin-style Merkle
padding otherwise admits duplicate-list ambiguity (CVE-2012-2459). The range length
comes from the independently authenticated snapshot height and the network's
activation constant, not from provider-reported tip or stream completion.

The commitment rules are specified in [ZIP 244](https://zips.z.cash/zip-0244) and
the Ironwood extension in [ZIP 229](https://zips.z.cash/zip-0229). The implementation
uses pinned `zakura-chain` parsing, hashing, and Merkle code: version `6.1.0`
for the default Zakura backend and version `5.0.0` for the upstream backend.
Both support Ironwood and are tested against the same mainnet fixtures. It does not
define a new hash function or commitment format.

This is **content verification relative to an authenticated history**, not full
consensus validation. It does not discover the best chain, validate difficulty or
proof of work, check all transaction consensus rules, or verify signatures and
zero-knowledge proofs. V5/V6 authorizing data is not committed by the transaction
ID, so changes confined to that data may pass; the guarantee concerns transaction
effects and Ironwood nullifiers. The caller's trusted node/channel supplies the
accepted-chain assumption. A provider can withhold data and prevent completion,
but cannot cause a partial history to be reported as successfully verified.

The resulting root uses the existing dataset-version-2 tree algorithm: sort and
deduplicate nullifiers, add protocol sentinels and padding, construct the depth-19
PIR tree, then extend to the depth-29 circuit root. The root commits to that
normalized tree, not transaction order or action occurrence counts. The report's
`ironwood_actions` counts occurrences before normalization. See
[the tree specification](pir-tree-spec.md) for the exact layout.

## Invocation

```bash
cargo +1.91.0 build --locked -p nf-server

target/debug/nf-server verify-root \
  --zcash-network main \
  --height 3428150 \
  --trusted-block-hash 00000000004c828d2e62a4782c9668cc794e318f5b214a90f29322c925a3b553 \
  --expected-circuit-root 79a308958ff5fe3f0e17a1688b4151fc9a59cca8ce3b7d7de8d77d0c5814893b \
  --block-rpc-url http://127.0.0.1:8232 \
  --block-rpc-cookie-file /path/to/node/.cookie
```

This historical tuple is a regression example, not an anchor for a different
snapshot. For new snapshots, independently authenticate their tuples first.

| Argument | Contract |
| --- | --- |
| `--zcash-network` | Required; `main` or `test`; no environment fallback. |
| `--height` | Required, at/after Ironwood activation and divisible by ten. Never clamped to a tip. |
| `--trusted-block-hash` | Required 32-byte hex block hash in RPC display order. |
| `--expected-circuit-root` | Required canonical 32-byte Pallas field hex, exactly the `circuit_root` encoding in `pir_root.json`; **not** `pir_root` and not a block hash. No modular reduction of invalid inputs. |
| `--block-rpc-url` | Required explicit HTTP(S) raw-block RPC. No `LWD_URLS` or standard sync overrides. URL credentials and fragments are rejected; redirects are disabled. |
| `--block-rpc-cookie-file` | Optional UTF-8 `user:password` cookie, read once. Keep credentials in the node's cookie file; do not embed them in URLs or command arguments. |
| `--http-timeout-secs` | Positive per-attempt timeout; default 30 seconds. |

The provider must retain raw blocks for the **entire** activation-to-snapshot
range. A fully synced node may have pruned historical bodies. A provider that
serves only compact blocks is insufficient. A trusted hash at the beginning of
the range alone is also insufficient to authenticate the subsequent history.

## Output, failures, and resource use

Progress and tracing go to stderr. Stdout contains one JSON document only after
the complete history has been verified and the tree has been built:

```json
{
  "zcash_network": "main",
  "nullifier_pool": "ironwood",
  "dataset_version": 2,
  "height": 3428150,
  "trusted_block_hash": "00000000004c828d2e62a4782c9668cc794e318f5b214a90f29322c925a3b553",
  "expected_circuit_root": "79a308958ff5fe3f0e17a1688b4151fc9a59cca8ce3b7d7de8d77d0c5814893b",
  "computed_circuit_root": "79a308958ff5fe3f0e17a1688b4151fc9a59cca8ce3b7d7de8d77d0c5814893b",
  "computed_pir_root": "e4c6ed4495514613ee8b04e7678423ba391f6cb22faf1dcef1ae82a9825edf04",
  "verified_blocks": 8,
  "ironwood_actions": 43,
  "matches": true
}
```

- Root mismatch prints the completed report with `matches: false` and exits 1.
- Invalid/incomplete data, unavailable history, invalid semantic arguments, and
  operational failures exit 1 without a result document. CLI syntax errors exit 2.
- Fetches are sequential. Transient transport failures, HTTP 429, and HTTP 5xx
  receive at most three total attempts, with 250 ms and 500 ms backoffs. A JSON-RPC
  error in a successful HTTP response, bad encoding, or failed commitment is not
  retried. RPCs returning application errors as HTTP 5xx follow the bounded HTTP
  retry policy. No failed block is skipped.
- Raw blocks are limited to the parser's 2,000,000-byte maximum. HTTP bodies are
  capped at twice that size plus 65,536 bytes for the JSON envelope, even without
  `Content-Length`. Provider error text and URL-bearing transport errors are not
  echoed, to prevent accidental credential disclosure.
- V1 holds action nullifiers and the resulting tree in memory. It caps action
  occurrences at 1,048,576; the tree builder also enforces its exact range capacity
  after sentinel insertion. There is no resume cache, disk output, or partial
  result. Larger datasets require a future explicit capacity/layout revision.

`sync`, `serve`, lightwalletd protobufs, checkpoint files, and tier formats are
unchanged. The new RPC transport runs only when `verify-root` is invoked. The
always-available command adds parser dependencies to builds. Parser selection follows the
existing mutually exclusive `zakura` and `upstream` features, preserving the
workspace's crypto dependency separation. Neither backend adds verification work
to standard ingestion.

## Mainnet validation: 2026-09-10

Validated the working-tree implementation based on commit
`d75ce485a5005a87727db560991f27cf0b72f99b`, using Rust 1.91.0 on macOS arm64.

- Range: mainnet `3,428,143..=3,428,150`, the complete Ironwood history through this
  early snapshot, not a sampled subset of a later snapshot.
- Raw source: the archival zcashd RPC at `[::1]:8232` on
  `roman-zcashd-compat-mainnet`, accessed through authenticated SSH. Credentials
  were read only on that host. The live CLI run used a temporary loopback HTTP
  bridge forwarding each `getblock(hash, 0)` through SSH, not fixture replay.
- Anchor cross-check: a separate mainnet node at `104.131.184.123:8232` returned the
  same snapshot hash. Both nodes are operator infrastructure; this is not a claim
  of an independent-organization quorum.
- Independent extraction: existing compact ingestion against `https://zec.rocks:443`
  in a fresh directory returned 43 Ironwood nullifiers. All 43 canonical bytes
  agree with extraction from the raw fixtures, including multiplicity.
- The live raw-block verifier returned exit 0, `matches: true`, eight verified
  blocks, and 43 actions. Both computed roots matched the independent compact
  export. The initial live run took 20.998 seconds, dominated by per-block SSH
  setup; this is not a throughput benchmark.
- A second live run with an all-zero proposed root returned exit 1 and
  `matches: false` after checking the same eight blocks (23.937 seconds).
- Manual CLI replay against deliberately modified raw responses rejected a
  removed transaction, a removed Ironwood action with a structurally valid
  remaining bundle, and a changed nullifier with a transaction Merkle-root
  mismatch. Withholding height `3,428,147` also returned exit 1. None of those
  failures emitted a result document; an unmodified replay returned exit 0.
- The final block contains 12 Ironwood and 14 Orchard actions. Its transaction IDs
  and Merkle root were separately recorded from the archival node; its Ironwood
  nullifiers were recorded from `zec.rocks` through `GetBlock`.

The independent compact reproduction command was:

```bash
LWD_URLS=https://zec.rocks:443 target/release/nf-server sync \
  --zcash-network main --max-height 3428150 \
  --pir-data-dir target/verify-root-manual/compact \
  --voting-config-url '' --non-interactive
```

Always use a fresh directory and check the exported height: standard `sync`
retains its existing tip-capping and cache behavior. This validation used the
pre-change release binary for the independent compact path.

Offline fixtures and provenance are in
[`nf-server/tests/fixtures/verify-root`](../nf-server/tests/fixtures/verify-root/README.md).
The CLI acceptance tests replay all eight real blocks and also supply forged
responses with omitted actions, omitted transactions, unavailable intermediate
blocks, wrong blocks, truncated data, and oversized responses. These must return
nonzero without a result document. An incorrect proposed root must return
`matches: false`; transient failures must retry without shortening the history.

```bash
cargo +1.91.0 test --locked -p nf-server -p nf-ingest -p pir-export -p imt-tree
cargo +1.91.0 test --locked -p nf-server --no-default-features --features upstream
cargo +1.91.0 clippy --locked -p nf-server --all-targets -- -D warnings
cargo +1.91.0 fmt --all -- --check
```

These checks do not automatically approve or publish a proposed root. Consumers
must explicitly require and authenticate the verification outcome before using
the root as a trusted proof input. Optional auditing alone does not close H3 for
the existing unverified ingest path.

The initial implementation (using parser version 5.0.0) passed 96 tests across `nf-server`, `nf-ingest`, `pir-export`,
and `imt-tree` with the default backend, plus 23 `nf-server` tests with the
upstream backend. Clippy with warnings denied, formatting, and the `serve` feature
build check passed on Rust 1.91.0. The
[machine-readable evidence](validation/verify-root-mainnet-2026-09-10.json)
records source-file fingerprints and the manual outcomes.


## Production snapshot validation

The [production validation record](validation/verify-root-production-3459350-2026-09-10.json)
records a full verification of the **NU7 Scope** round at snapshot height
**3,459,350**: **31,208 blocks** and **101,583 Ironwood action occurrences**.
Both the reconstructed circuit root and PIR root matched the published values.
The active-round identity and roots were checked again after completion.

The snapshot height is the **end** of the verified range. Ironwood activation at
**3,428,143** is its beginning; the activation block's hash alone cannot
authenticate the blocks that follow it.

Validation records identify the exact source and binary fingerprints used. They
are historical evidence, not an assertion that every later code revision or
production round has been verified.
