//! Coordinator authorization for host updates. Trust is compiled in, never fetched.
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const KEYS: &str = include_str!("../pir-update-keys.json");

/// Artifact digests authenticated together with the exact PIR configuration bytes.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    /// SHA-256 of the exact published pir.json bytes.
    pub config_sha256: String,
    /// SHA-256 of the Linux amd64 executable.
    pub linux_amd64_sha256: String,
    /// SHA-256 of the Linux arm64 executable.
    pub linux_arm64_sha256: String,
    /// SHA-256 of the snapshot manifest that authenticates all snapshot files.
    pub snapshot_manifest_sha256: String,
    /// SHA-256 of the systemd service unit.
    pub service_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attestations {
    schema_version: u32,
    payload: Payload,
    signatures: Vec<SignatureRef>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignatureRef {
    key_id: String,
    alg: String,
    sig: String,
}
#[derive(Deserialize)]
struct Key {
    key_id: String,
    pubkey: String,
}
/// Desired binary and snapshot selected by the coordinator.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// PIR configuration schema version; currently 1.
    pub schema_version: u32,
    /// Exact snapshot height to serve.
    pub snapshot_height: u64,
    /// Explicit release tag whose bytes are bound by the signed payload.
    pub binary_tag: String,
}

/// Fixed field order and lowercase hex make this encoding unambiguous across languages.
pub fn signing_bytes(scope: &str, payload: &Payload) -> Result<Vec<u8>> {
    ensure!(matches!(scope, "prod" | "stage"), "invalid update scope");
    let mut result = format!("valargroup/pir-update/v1\n{scope}\n");
    for hash in [
        &payload.config_sha256,
        &payload.linux_amd64_sha256,
        &payload.linux_arm64_sha256,
        &payload.snapshot_manifest_sha256,
        &payload.service_sha256,
    ] {
        ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid SHA-256"
        );
        result.push_str(hash);
        result.push('\n');
    }
    Ok(result.into_bytes())
}

fn verify_with_keys(
    config: &[u8],
    attestations: &[u8],
    scope: &str,
    keys: &[Key],
) -> Result<(Config, Payload)> {
    ensure!(
        config.len() <= 65536 && attestations.len() <= 65536,
        "update metadata too large"
    );
    let att: Attestations = serde_json::from_slice(attestations).context("decode attestations")?;
    ensure!(att.schema_version == 1, "unsupported attestation version");
    let message = signing_bytes(scope, &att.payload)?;
    ensure!(
        hex::encode(Sha256::digest(config)) == att.payload.config_sha256,
        "config does not match attestation"
    );
    let valid = att.signatures.iter().any(|sig| {
        if sig.alg != "ed25519" {
            return false;
        }
        keys.iter()
            .filter(|key| key.key_id == sig.key_id)
            .any(|key| {
                let Ok(pk) = STANDARD.decode(&key.pubkey) else {
                    return false;
                };
                let Ok(pk): Result<[u8; 32], _> = pk.try_into() else {
                    return false;
                };
                let Ok(pk) = VerifyingKey::from_bytes(&pk) else {
                    return false;
                };
                let Ok(sig) = STANDARD.decode(&sig.sig) else {
                    return false;
                };
                let Ok(sig) = Signature::from_slice(&sig) else {
                    return false;
                };
                pk.verify_strict(&message, &sig).is_ok()
            })
    });
    ensure!(
        valid,
        "no trusted coordinator signature matches this update"
    );
    let cfg: Config = serde_json::from_slice(config).context("decode PIR config")?;
    ensure!(cfg.schema_version == 1, "unsupported PIR config version");
    let tag = &cfg.binary_tag;
    ensure!(
        tag.starts_with('v')
            && tag.len() <= 128
            && tag.len() > 1
            && tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b)),
        "invalid explicit release tag"
    );
    ensure!(
        cfg.snapshot_height > 0 && cfg.snapshot_height <= 9_007_199_254_740_991,
        "invalid snapshot height"
    );
    Ok((cfg, att.payload))
}

/// Files and installed trust scope for a host-side verification request.
#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]
    scope: String,
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    attestations: PathBuf,
    #[arg(long)]
    zcash_network: pir_types::ZcashNetwork,
}
/// Verify metadata against compiled keys and the installed network, then print JSON.
pub fn run(args: Args) -> Result<()> {
    let keys: std::collections::BTreeMap<String, Vec<Key>> = serde_json::from_str(KEYS)?;
    let Some(keys) = keys.get(&args.scope) else {
        bail!("unknown scope")
    };
    for path in [&args.config, &args.attestations] {
        ensure!(
            std::fs::metadata(path)?.len() <= 65536,
            "update metadata too large"
        );
    }
    let (cfg, payload) = verify_with_keys(
        &std::fs::read(args.config)?,
        &std::fs::read(args.attestations)?,
        &args.scope,
        keys,
    )?;
    nf_ingest::config::validate_export_height(cfg.snapshot_height, args.zcash_network)?;
    println!("{}", serde_json::json!({"config":cfg,"payload":payload}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    #[test]
    fn cross_language_known_answer_and_malformed_inputs() {
        let vector: serde_json::Value =
            serde_json::from_str(include_str!("../../testdata/pir-update-vector.json")).unwrap();
        let cfg = vector["config"].as_str().unwrap().as_bytes();
        let keys: Vec<Key> = vec![serde_json::from_value(vector["key"].clone()).unwrap()];
        let p: Payload = serde_json::from_value(vector["payload"].clone()).unwrap();
        assert_eq!(
            STANDARD.encode(signing_bytes("prod", &p).unwrap()),
            vector["message_base64"]
        );
        let att = serde_json::to_vec(&vector["attestations"]).unwrap();
        assert!(verify_with_keys(cfg, &att, "prod", &keys).is_ok());
        for malformed in ["", "!!!", "AA==", &"A".repeat(88)] {
            let mut a = vector["attestations"].clone();
            a["signatures"][0]["sig"] = malformed.into();
            assert!(
                verify_with_keys(cfg, &serde_json::to_vec(&a).unwrap(), "prod", &keys).is_err()
            );
        }
        assert!(verify_with_keys(cfg, &att, "other", &keys).is_err());
        assert!(verify_with_keys(&vec![b' '; 65537], &att, "prod", &keys).is_err());
        let bad_keys = [Key {
            key_id: "fixture".into(),
            pubkey: "AA==".into(),
        }];
        assert!(verify_with_keys(cfg, &att, "prod", &bad_keys).is_err());
        let mut bad = p.clone();
        bad.config_sha256 = "A".repeat(64);
        assert!(signing_bytes("prod", &bad).is_err());
    }

    #[test]
    fn signature_binds_config_scope_and_every_artifact() {
        let sk = SigningKey::from_bytes(&[7; 32]); // public test fixture only
        let keys = [Key {
            key_id: "test".into(),
            pubkey: STANDARD.encode(sk.verifying_key().as_bytes()),
        }];
        let cfg = br#"{"schema_version":1,"snapshot_height":3484440,"binary_tag":"v1.2.3"}"#;
        let p = Payload {
            config_sha256: hex::encode(Sha256::digest(cfg)),
            linux_amd64_sha256: "1".repeat(64),
            linux_arm64_sha256: "2".repeat(64),
            snapshot_manifest_sha256: "3".repeat(64),
            service_sha256: "4".repeat(64),
        };
        let mut a = serde_json::json!({"schema_version":1,"payload":p,"signatures":[{"key_id":"test","alg":"ed25519","sig":STANDARD.encode(sk.sign(&signing_bytes("prod", &p).unwrap()).to_bytes())}]});
        let raw = serde_json::to_vec(&a).unwrap();
        assert!(verify_with_keys(cfg, &raw, "prod", &keys).is_ok());
        assert!(verify_with_keys(cfg, &raw, "stage", &keys).is_err());
        assert!(verify_with_keys(cfg, &raw, "prod", &[]).is_err());
        assert!(verify_with_keys(&[cfg.as_slice(), b"\n"].concat(), &raw, "prod", &keys).is_err());
        for field in [
            "linux_amd64_sha256",
            "linux_arm64_sha256",
            "snapshot_manifest_sha256",
            "service_sha256",
        ] {
            let mut changed = a.clone();
            changed["payload"][field] = "f".repeat(64).into();
            assert!(
                verify_with_keys(cfg, &serde_json::to_vec(&changed).unwrap(), "prod", &keys)
                    .is_err()
            );
        }
        a["schema_version"] = 2.into();
        assert!(verify_with_keys(cfg, &serde_json::to_vec(&a).unwrap(), "prod", &keys).is_err());
    }
}
