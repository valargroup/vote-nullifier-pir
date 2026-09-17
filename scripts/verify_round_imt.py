#!/usr/bin/env python3
"""Rebuild the IMT committed by an explicitly selected or latest voting round.

Uses svoted's canonical round-ID check and the normal PIR sync/build pipeline.
Raw-block verification remains available with --mode raw-blocks.
No wallet, signing key, service mutation, or report import is involved.
"""

import argparse
import datetime
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.parse import urlsplit


class VerificationError(Exception):
    """An incomplete verification that must never be reported as a match."""


IDENTITY_FIELDS = (
    "chain_id", "round_id", "created_at_height", "snapshot_height",
    "snapshot_blockhash", "proposals_hash", "vote_end_time", "nullifier_imt_root", "nc_root",
)
HEX_FIELDS = ("round_id", "snapshot_blockhash", "proposals_hash", "nullifier_imt_root", "nc_root")


def invoke(command, progress=False):
    # No shell evaluation. nf-server sanitizes its RPC errors and prints progress
    # to stderr. Query errors may contain private URLs, so do not echo them.
    try:
        proc = subprocess.run(command, stdout=subprocess.PIPE,
                              stderr=None if progress else subprocess.PIPE,
                              text=True, check=False)
    except OSError as error:
        raise VerificationError(f"Unable to run {Path(command[0]).name}. Check the installed tool and path.") from error
    try:
        result = json.loads(proc.stdout)
    except (ValueError, TypeError) as error:
        message = f"{Path(command[0]).name} did not return a completed JSON result."
        # These errors are produced by the round selector and contain only IDs.
        for line in (proc.stderr or "").splitlines():
            if line.startswith("Error: no registered voting rounds") or line.startswith("Error: multiple rounds at latest creation height"):
                message = line.removeprefix("Error: ")
                break
        raise VerificationError(message) from error
    if not isinstance(result, dict):
        raise VerificationError("Tool output must be a JSON object.")
    return proc.returncode, result


def query_round(args, round_id=None):
    selector = [round_id] if round_id else ["--latest"]
    code, result = invoke([args.svoted, "query", "vote", "verify-round", *selector,
                           "--node", args.vote_node, "--expected-chain-id", args.vote_chain_id,
                           "--output", "json"])
    if code != 0 or result.get("round_id_matches") is not True:
        raise VerificationError("The round-ID binding check failed.")
    if result.get("chain_id") != args.vote_chain_id:
        raise VerificationError("The voting node returned a different chain.")
    for field in HEX_FIELDS:
        if not isinstance(result.get(field), str) or not re.fullmatch(r"[0-9a-f]{64}", result[field]):
            raise VerificationError(f"Round query returned an invalid {field}.")
    for field in ("created_at_height", "snapshot_height", "vote_end_time"):
        if type(result.get(field)) is not int or not 0 < result[field] <= (1 << 64) - 1:
            raise VerificationError(f"Round query returned an invalid {field}.")
    if result.get("computed_round_id") != result["round_id"] or (round_id and result["round_id"] != round_id):
        raise VerificationError("The queried round does not match its expected identity.")
    return result


def fingerprint(executable):
    resolved = shutil.which(executable)
    if not resolved:
        raise VerificationError(f"Required command unavailable: {Path(executable).name}")
    digest = hashlib.sha256()
    with open(resolved, "rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def rebuild_with_pir_sync(args, selected):
    # Always rebuild from fresh input. Never reuse or clear a service's data.
    with tempfile.TemporaryDirectory(prefix="round-imt-") as directory:
        command = [args.nf_server, "sync", "--zcash-network", args.zcash_network,
                   "--max-height", str(selected["snapshot_height"]),
                   "--lwd-url", args.lwd_url, "--pir-data-dir", directory,
                   "--output-dir", directory, "--voting-config-url", "", "--non-interactive"]
        env = os.environ.copy()
        # LWD_URLS takes precedence over --lwd-url in the normal sync command.
        env["LWD_URLS"] = args.lwd_url
        env.pop("SVOTE_PIR_SYNC_RESET", None)
        env.pop("SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH", None)
        try:
            proc = subprocess.run(command, env=env, stdout=sys.stderr, stderr=sys.stderr, check=False)
        except OSError as error:
            raise VerificationError("Unable to run nf-server sync. Check the installed tool and path.") from error
        if proc.returncode != 0:
            raise VerificationError("PIR sync did not complete. No root match was established.")
        try:
            metadata = json.loads((Path(directory) / "pir_root.json").read_text())
        except (OSError, ValueError) as error:
            raise VerificationError("PIR sync did not produce readable root metadata.") from error
        expected = {"zcash_network": args.zcash_network, "nullifier_pool": "ironwood",
                    "dataset_version": 2, "height": selected["snapshot_height"]}
        if not isinstance(metadata, dict) or any(metadata.get(key) != value for key, value in expected.items()):
            raise VerificationError("PIR output does not match the exact snapshot height, network, or dataset.")
        # sync may cap its target to the node's tip. Metadata must match the
        # requested height before comparing roots, even if the roots are equal.
        return {**expected, "computed_circuit_root": metadata.get("circuit_root"),
                "computed_pir_root": metadata.get("pir_root"), "lwd_url": args.lwd_url}


def rebuild_with_raw_blocks(args, selected):
    command = [args.nf_server, "verify-root", "--zcash-network", args.zcash_network,
               "--height", str(selected["snapshot_height"]), "--trusted-block-hash", args.trusted_block_hash,
               "--expected-circuit-root", selected["nullifier_imt_root"], "--block-rpc-url", args.block_rpc_url]
    if args.block_rpc_cookie_file:
        command += ["--block-rpc-cookie-file", args.block_rpc_cookie_file]
    code, rebuild = invoke(command, progress=True)
    expected = {"zcash_network": args.zcash_network, "nullifier_pool": "ironwood", "dataset_version": 2,
                "height": selected["snapshot_height"], "trusted_block_hash": args.trusted_block_hash,
                "expected_circuit_root": selected["nullifier_imt_root"]}
    if any(rebuild.get(key) != value for key, value in expected.items()):
        raise VerificationError("Verifier output does not match the selected snapshot or dataset.")
    if type(rebuild.get("verified_blocks")) is not int or rebuild["verified_blocks"] <= 0:
        raise VerificationError("Verifier did not report a completed block range.")
    matches = rebuild.get("computed_circuit_root") == selected["nullifier_imt_root"]
    if rebuild.get("matches") is not matches or (code == 0) != matches:
        raise VerificationError("Verifier exit status and root comparison disagree.")
    return rebuild


def verify(args):
    selected = query_round(args, args.round_id)
    if args.inspect:
        return {"outcome": "inspection", "round": selected, "imt_rebuild": "not performed"}
    if args.mode == "pir-sync":
        if args.trusted_block_hash or args.block_rpc_url or args.block_rpc_cookie_file:
            raise VerificationError("Raw-block options require --mode raw-blocks.")
        if not args.lwd_url:
            raise VerificationError("PIR rebuilding requires an explicit --lwd-url.")
    else:
        if args.lwd_url:
            raise VerificationError("--lwd-url is only used by --mode pir-sync.")
        if not args.trusted_block_hash or not args.block_rpc_url:
            raise VerificationError("Raw-block verification requires --trusted-block-hash and --block-rpc-url. Use --inspect to discover the snapshot first.")
    result = {"outcome": "incomplete", "method": args.mode, "round": selected, "zcash_network": args.zcash_network}
    if args.mode == "raw-blocks" and selected["snapshot_blockhash"] != args.trusted_block_hash:
        result.update(outcome="mismatch", failure="The independently trusted hash differs from the on-chain snapshot hash.",
                      trusted_block_hash=args.trusted_block_hash)
        return result
    print(f"Rebuilding round {selected['round_id']} ({selected.get('status', 'unknown status')}) at Zcash height {selected['snapshot_height']} using {args.mode}", file=sys.stderr)
    identity = {"svoted_sha256": fingerprint(args.svoted), "nf_server_sha256": fingerprint(args.nf_server)}
    info_code, info = invoke([args.nf_server, "build-info", "--json"])
    if info_code != 0:
        raise VerificationError("Unable to read verifier build identity.")
    identity["nf_server_build"] = info
    rebuild = rebuild_with_pir_sync(args, selected) if args.mode == "pir-sync" else rebuild_with_raw_blocks(args, selected)
    computed = rebuild.get("computed_circuit_root")
    if not isinstance(computed, str) or not re.fullmatch(r"[0-9a-f]{64}", computed):
        raise VerificationError("Verifier returned an invalid computed root.")
    result.update(rebuild=rebuild, tools=identity)
    if args.mode == "pir-sync":
        result["trust"] = "Reproduces PIR's result using trusted lightwalletd data and the same tree implementation. Does not authenticate raw blocks."
    matches = computed == selected["nullifier_imt_root"]
    rebuild["matches"] = matches
    if not matches:
        result.update(outcome="mismatch", failure="The rebuilt circuit root differs from the on-chain IMT root.")
        return result
    current = query_round(args, selected["round_id"])
    if any(current[field] != selected[field] for field in IDENTITY_FIELDS):
        result["failure"] = "The selected round's fields changed during verification."
        return result
    result["outcome"] = "verified"
    if not args.round_id:
        try:
            latest = query_round(args)
            if latest["round_id"] != selected["round_id"]:
                result["notice"] = f"A newer round is now available: {latest['round_id']}. This result covers only the round shown above."
        except VerificationError:
            result["notice"] = "The pinned round was verified, but latest-round selection could not be refreshed."
    return result


def print_result(result):
    print(f"Result: {result['outcome'].upper()}")
    round_info = result.get("round", {})
    if round_info:
        print(f"Round: {round_info['round_id']}\nVoting chain: {round_info['chain_id']}\nStatus: {round_info.get('status', 'unknown')}")
        print(f"Voting query height: {round_info.get('query_height', 'unknown')}\nRound creation height: {round_info['created_at_height']}")
        print(f"Zcash snapshot height: {round_info['snapshot_height']}\nSnapshot hash: {round_info['snapshot_blockhash']}")
        print(f"On-chain IMT root: {round_info['nullifier_imt_root']}\nRound-ID binding: matched")
    if "zcash_network" in result:
        print(f"Zcash network: {result['zcash_network']}")
    if "rebuild" in result:
        print(f"Method: {result['method']}\nComputed IMT root: {result['rebuild']['computed_circuit_root']}")
        print(f"Root comparison: {'MATCH' if result['rebuild']['matches'] else 'MISMATCH'}")
        if "verified_blocks" in result["rebuild"]:
            print(f"Verified raw blocks: {result['rebuild']['verified_blocks']}")
        if "lwd_url" in result["rebuild"]:
            print(f"Lightwalletd source: {result['rebuild']['lwd_url']}")
    if "tools" in result:
        print(f"Verifier: {json.dumps(result['tools']['nf_server_build'], sort_keys=True)}")
        print(f"svoted SHA-256: {result['tools']['svoted_sha256']}\nnf-server SHA-256: {result['tools']['nf_server_sha256']}")
    for key in ("imt_rebuild", "trusted_block_hash", "trust", "failure", "notice", "completed_at"):
        if key in result:
            print(f"{key}: {result[key]}")


def parser():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--vote-node", required=True, help="Trusted voting-chain CometBFT RPC")
    p.add_argument("--vote-chain-id", required=True)
    selection = p.add_mutually_exclusive_group()
    selection.add_argument("--latest", action="store_true", help="Newest registered round, regardless of approval (default)")
    selection.add_argument("--round-id", type=lambda value: hex32(value))
    p.add_argument("--zcash-network", required=True, choices=("main", "test"))
    p.add_argument("--mode", choices=("pir-sync", "raw-blocks"), default="pir-sync",
                   help="Normal PIR rebuild (default), or authenticated raw-block verification")
    p.add_argument("--lwd-url", help="Trusted lightwalletd endpoint for the normal PIR rebuild")
    p.add_argument("--trusted-block-hash", type=lambda value: hex32(value))
    p.add_argument("--block-rpc-url")
    p.add_argument("--block-rpc-cookie-file")
    p.add_argument("--svoted", default="svoted", help="Path to svoted with the verify-round query")
    p.add_argument("--nf-server", default="nf-server")
    p.add_argument("--inspect", action="store_true", help="Show the selected round and snapshot without rebuilding")
    p.add_argument("--json", action="store_true", help="Print the result as JSON instead of a human summary")
    return p


def hex32(value):
    if not re.fullmatch(r"[0-9a-fA-F]{64}", value):
        raise argparse.ArgumentTypeError("expected exactly 32 bytes of hex")
    return value.lower()


def main():
    args = parser().parse_args()
    try:
        for endpoint in (args.vote_node, args.block_rpc_url, args.lwd_url):
            if endpoint:
                url = urlsplit(endpoint)
                if url.scheme not in ("http", "https", "tcp") or not url.hostname or url.username or url.password or url.query or url.fragment:
                    raise VerificationError("RPC URLs must have a host and contain no credentials, query, or fragment.")
        result = verify(args)
    except (VerificationError, OSError, ValueError) as error:
        # Exception text from our verifier is safe; OS/URL errors may include secrets.
        message = str(error) if isinstance(error, VerificationError) else "Verification could not complete. Check local tools and RPC configuration."
        result = {"outcome": "incomplete", "failure": message}
    result["completed_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print_result(result)
    return 0 if result["outcome"] in ("verified", "inspection") else 1


if __name__ == "__main__":
    sys.exit(main())
