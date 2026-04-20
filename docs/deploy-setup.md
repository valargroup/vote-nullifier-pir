# Deploy setup for nf-server

This guide covers two deployment paths:

- **[Binary setup](#binary-setup-operators)** -- Download a pre-built binary and run the service. No Rust toolchain or git clone required.
- **[Source setup](#source-setup-developers)** -- Build from source with CI/CD-driven deployment.

---

## Hardware requirements

| Resource | Minimum | Recommended | Notes |
|----------|---------|-------------|-------|
| **CPU** | x86-64 (any) | x86-64 with AVX-512 | AVX-512 gives ~2x query throughput. Intel Ice Lake / Sapphire Rapids or newer. AMD Zen 4+. |
| **RAM** | 16 GB | 32 GB | The server loads ~6 GB of tier data and builds YPIR internal structures. Peak usage during initialization is roughly 2x the tier data size. |
| **Disk** | 20 GB free | 40 GB free | Nullifier data (~1.6 GB), PIR tier files (~6 GB), plus headroom for ingestion and re-export. |
| **OS** | Linux (x86-64) | Ubuntu 22.04+ / Debian 12+ | macOS (arm64/amd64) binaries are also published but not recommended for production serving. |
| **Network** | Outbound HTTPS | Static IP or DNS A record | Needs outbound access to a lightwalletd gRPC endpoint for ingestion. Inbound access on the serve port for clients. |

### AVX-512 note

The `serve` feature works on any x86-64 CPU. AVX-512 is an optional optimization that approximately halves PIR query latency (tier 1: ~0.5 s, tier 2: ~1.6 s per query). The pre-built `linux-amd64` release binary includes AVX-512 support; on CPUs without it the binary still runs but falls back to baseline SIMD.

---

## Binary setup (operators)

This path is for operators who want to run `nf-server` without cloning the repository or installing the Rust toolchain.

### 1. Download the binary

Grab the latest release from GitHub:

```bash
# Pick the asset for your platform
PLATFORM="linux-amd64"   # or: linux-arm64, darwin-amd64, darwin-arm64
VERSION=$(curl -s https://api.github.com/repos/valargroup/vote-nullifier-pir/releases/latest | grep tag_name | cut -d'"' -f4)

sudo mkdir -p /opt/nf-ingest
cd /opt/nf-ingest

# Download the binary and systemd unit
curl -fLO "https://github.com/valargroup/vote-nullifier-pir/releases/download/${VERSION}/nf-server-${PLATFORM}"
curl -fLO "https://github.com/valargroup/vote-nullifier-pir/releases/download/${VERSION}/nullifier-query-server.service"

sudo mv "nf-server-${PLATFORM}" nf-server
sudo chmod +x nf-server
```

### 2. Configure the snapshot bootstrap

Tell `nf-server` where to find the published voting-config and the
pre-computed snapshot bucket. Both have production defaults baked into
the binary; pinning them here keeps the deployment self-documenting and
lets you redirect to a staging mirror without rebuilding.

```bash
sudo tee /etc/default/nf-server <<'EOF'
SVOTE_VOTING_CONFIG_URL=https://valargroup.github.io/token-holder-voting-config/voting-config.json
SVOTE_PRECOMPUTED_BASE_URL=https://vote.fra1.digitaloceanspaces.com
EOF
```

That is the entire bootstrap step. On startup, `nf-server` reads
`voting-config.snapshot_height` and downloads
`<bucket>/snapshots/<height>/{manifest.json,tier*.bin,pir_root.json}`,
verifies sha256 against the manifest, and atomically swaps into
`/opt/nf-ingest/pir-data/`. There is no manual ingest, no manual
export, and no separate cron-driven re-sync — the next bump is just a
config PR plus a `systemctl restart` (see [the runbook][runbook]).

> The legacy first-boot flow (`curl` the raw `nullifiers.{bin,checkpoint,tree}`,
> run `nf-server ingest`, then `nf-server export`) still works on
> offline / dev machines: set `SVOTE_VOTING_CONFIG_URL=` (empty string)
> and the binary will skip the bootstrap and serve whatever is on disk.

[runbook]: https://valargroup.github.io/shielded-vote-book/operations/snapshot-bumps.html

### 3. Install the systemd service

```bash
sudo cp /opt/nf-ingest/nullifier-query-server.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable nullifier-query-server
sudo systemctl start nullifier-query-server
```

Verify the service is running and serving the expected snapshot:

```bash
sudo systemctl status nullifier-query-server
curl http://localhost:3000/health
curl -s http://localhost:3000/root | jq .
curl -s http://localhost:3000/metrics | grep -E 'nf_snapshot_(served|expected)_height'
```

---

## Caddy reverse proxy with automatic TLS

[Caddy](https://caddyserver.com/) provides automatic HTTPS certificate provisioning via Let's Encrypt. This section sets up Caddy in front of `nf-server` so clients connect over TLS.

### Prerequisites

- A domain name with a DNS A record pointing to your server's public IP.
- Ports 80 and 443 open in your firewall (Caddy needs both for ACME HTTP-01 challenge).

### Install Caddy

```bash
# Debian / Ubuntu
sudo apt install -y debian-keyring debian-archive-keyring apt-transport-https
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo apt update
sudo apt install caddy
```

### Configure Caddy

Replace `pir.example.com` with your actual domain:

```bash
cat <<'EOF' | sudo tee /etc/caddy/Caddyfile
pir.example.com {
    reverse_proxy localhost:3000
}
EOF
```

### Start Caddy

```bash
sudo systemctl enable caddy
sudo systemctl restart caddy
```

Caddy will automatically obtain and renew a TLS certificate. Verify with:

```bash
curl https://pir.example.com/health
```

---

## Source setup (developers)

This path is for contributors and operators who want to build from source with CI/CD-driven deployment.

### Moving cached data to the deploy directory

The service uses flat binary files for nullifier storage. To move them into the deploy directory (default `/opt/nf-ingest`):

```bash
sudo mkdir -p /opt/nf-ingest

# Stop the service first if it is running
sudo systemctl stop nullifier-query-server || true

# Move data files
sudo mv /path/to/nullifiers.bin        /opt/nf-ingest/
sudo mv /path/to/nullifiers.checkpoint /opt/nf-ingest/
sudo mv /path/to/nullifiers.tree       /opt/nf-ingest/

# Ensure the deploy user can write (if deploy runs as a different user)
# sudo chown -R DEPLOY_USER:DEPLOY_USER /opt/nf-ingest
```

The unit file in `docs/nullifier-query-server.service` uses `/opt/nf-ingest` as the data directory by default.

### GitHub repository secrets

The CI workflows use these repository secrets (**Settings > Secrets and variables > Actions**):

| Secret | Used by | Description |
|--------|---------|-------------|
| `PIR_PRIMARY_HOST` | `deploy.yml` | Hostname or IP of the PIR primary server. |
| `PIR_BACKUP_HOST` | `deploy.yml` | Hostname or IP of the PIR backup server. |
| `DEPLOY_HOST` | `resync.yml` | Hostname or IP of the resync target (typically the primary). |
| `DEPLOY_USER` | all | SSH username on the remote hosts. |
| `SSH_KEY` | all | SSH private key for authentication. |
| `NF_SENTRY_DSN` | `deploy.yml` | Sentry DSN written to `/opt/nf-ingest/.env` on deploy. |
| `DO_ACCESS_KEY` | `release.yml` | DigitalOcean Spaces access key (optional; for artifact mirroring). |
| `DO_SECRET_KEY` | `release.yml` | DigitalOcean Spaces secret key (optional). |

### One-time setup on the remote host

**Directory and binaries**

- Create the deploy directory. Default in the workflow is `DEPLOY_PATH: /opt/nf-ingest`.
- Ensure the SSH user can write to that directory.
- Either bootstrap the nullifier data (`make bootstrap`) or run an initial ingest.

**Query server (PIR HTTP API)**

The `nf-server serve` subcommand starts the PIR HTTP server. It needs:

- **PIR data**: Exported tier files in `pir-data/`. Either populated automatically by the startup self-bootstrap (default) or pre-staged manually via `nf-server export`.
- **Bootstrap config**: `SVOTE_VOTING_CONFIG_URL` and `SVOTE_PRECOMPUTED_BASE_URL` env vars (compiled-in defaults point at production). Set the former to an empty string to disable the bootstrap entirely.
- **Nullifier data** (only on the publisher host that runs `publish-snapshot.yml`): `nullifiers.bin` and `nullifiers.checkpoint` in `--data-dir`. PIR-only replicas no longer need these.
- **Port**: Configurable via `--port` (default 3000).

A systemd unit file is provided at `docs/nullifier-query-server.service`. Copy to `/etc/systemd/system/`:

```bash
sudo cp docs/nullifier-query-server.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable nullifier-query-server
sudo systemctl start nullifier-query-server
```

**Bumping to a new snapshot**

Edit `voting-config.json`'s `snapshot_height`, run
[`publish-snapshot.yml`](https://github.com/valargroup/vote-nullifier-pir/actions/workflows/publish-snapshot.yml)
for the new height, and rolling-restart the fleet. See the [operator
runbook][runbook] for the full procedure. The old per-host
`resync.yml` / `nf-resync.timer` flow is no longer required and was
removed from `vote-infrastructure/cloud-init/pir.yaml`.

### Changing deploy path or restart command

- **Deploy path**: Edit the `env.DEPLOY_PATH` in `.github/workflows/deploy.yml` (default `/opt/nf-ingest`).
- **Restart command**: Edit the "Install and restart" step in that workflow if you use a different service name.

### Manual runs

Both `deploy.yml` and `resync.yml` support `workflow_dispatch`, so you can trigger them from **Actions > Run workflow** without pushing to `main`.

### Test locally

From the workspace root:

```bash
# Bootstrap nullifier data (first run only)
make bootstrap

# Or ingest from scratch
make ingest

# Export PIR tier files
make export-nf

# Start the server
make serve
```

Then check `http://localhost:3000/health` and `http://localhost:3000/root`.

---

## CI/CD workflows

```mermaid
flowchart LR
    tag["git tag v*"] --> release["release.yml\nbuild + GitHub Release\n+ DO Spaces"]
    release --> deploy["deploy.yml\nSSH binary push\nto PIR hosts"]
    deploy --> health["health check\nlocalhost:3000/health"]
    manual["workflow_dispatch"] -.-> deploy
    resync["resync.yml\ningest + export + restart"] -.-> pirHost["PIR host"]
```

| Workflow | Trigger | What it does |
|----------|---------|-------------|
| [`release.yml`](https://github.com/valargroup/vote-nullifier-pir/blob/main/.github/workflows/release.yml) | `v*` tag push | Builds `nf-server` for linux/darwin x amd64/arm64, creates a GitHub Release with binaries + systemd unit, mirrors to DO Spaces, then automatically calls `deploy.yml`. |
| [`deploy.yml`](https://github.com/valargroup/vote-nullifier-pir/blob/main/.github/workflows/deploy.yml) | Called by `release.yml`, or manual `workflow_dispatch` | Downloads binary from GitHub Releases, SCPs to PIR hosts, writes `.env`, copies systemd unit, restarts service, runs health check. Supports deploying to primary, backup, or both. |
| [`publish-snapshot.yml`](https://github.com/valargroup/vote-nullifier-pir/blob/main/.github/workflows/publish-snapshot.yml) | Manual `workflow_dispatch` (with optional `height` input) | Runs ingest + export on `PIR_BACKUP_HOST`, builds `manifest.json`, uploads `s3://vote/snapshots/<height>/{tier*.bin,pir_root.json,manifest.json}` to DO Spaces, round-trip-verifies. Replicas pick up the new snapshot via the startup self-bootstrap on next restart. |
| [`resync.yml`](https://github.com/valargroup/vote-nullifier-pir/blob/main/.github/workflows/resync.yml) | Manual `workflow_dispatch` | **Legacy** ingest + export + restart on a single host. Superseded by `publish-snapshot.yml` plus `nf-server`'s startup self-bootstrap; kept for emergencies. |

---

## Infrastructure

PIR infrastructure (droplets, volumes, firewalls, DNS) is managed by Terraform in the
[vote-infrastructure](https://github.com/valargroup/vote-infrastructure) repo. Two
DigitalOcean droplets (primary + backup) sit in the `vote-sdk-vpc` VPC with Cloudflare
DNS records:

| Hostname | Droplet | Size |
|----------|---------|------|
| `pir-primary.<domain>` | `vote-nullifier-pir-primary` | `g-8vcpu-32gb-intel` (Premium Intel, AVX-512) |
| `pir-backup.<domain>` | `vote-nullifier-pir-backup` | `m-4vcpu-32gb-intel` (Premium Intel, AVX-512) |
| `pir.<domain>` | pir-primary (convenience alias) | -- |

Cloud-init templates in `vote-infrastructure/cloud-init/pir.yaml` handle first-boot
provisioning: install Caddy, mount the block volume, download `nf-server` from a
GitHub release, write `/etc/default/nf-server` with the bootstrap config
(`SVOTE_VOTING_CONFIG_URL`, `SVOTE_PRECOMPUTED_BASE_URL`), and start the service.
First-boot snapshot population and subsequent height bumps both go through
`nf-server`'s built-in self-bootstrap from the published bucket — there is no
longer a curl-based pre-stage step or a periodic `nf-resync.timer`. See the
[operator runbook][runbook] for the snapshot-bump procedure.

[runbook]: https://valargroup.github.io/shielded-vote-book/operations/snapshot-bumps.html
