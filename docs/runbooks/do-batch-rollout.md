# Runbook: DigitalOcean Batch Rollout Test Host

Use this runbook for a one-off DigitalOcean host that is benchmarked from a
local machine. This path does not require adding a staging target to CI.

## Recommended Instance

Use the production-like Premium Intel shape:

- Region: `fra1`
- Size: `m-4vcpu-32gb-intel`
- Image: `ubuntu-24-04-x64`
- Disk: attach a 100 GB ext4 volume, or use an equivalent VM with at least 35 GB
  free disk after OS install.
- Access: SSH key auth as `root` or a sudo-capable user.

The host should expose TCP `22` for SSH and TCP `3000` to the local benchmark
machine. HTTPS/Caddy is optional for this local-only measurement; the benchmark
commands below use `http://<host-ip>:3000`.

## Create Host

With `doctl` configured locally:

```bash
doctl compute droplet create vote-nullifier-pir-batch-test \
  --region fra1 \
  --image ubuntu-24-04-x64 \
  --size m-4vcpu-32gb-intel \
  --ssh-keys <ssh-key-id-or-fingerprint> \
  --wait

doctl compute droplet get vote-nullifier-pir-batch-test \
  --format ID,Name,PublicIPv4,Status
```

If you want a separate volume:

```bash
doctl compute volume create pir-batch-test-data \
  --region fra1 \
  --size 100GiB \
  --fs-type ext4

doctl compute volume-action attach pir-batch-test-data \
  --droplet-id <droplet-id> \
  --wait
```

Mount the volume at `/opt/nf-ingest` or leave the droplet disk in place if it has
enough space.

## Install Baseline

Install the baseline release first:

```bash
ssh root@<host-ip> 'curl -fsSL https://vote.fra1.digitaloceanspaces.com/start_pir.sh | sudo bash'
ssh root@<host-ip> 'systemctl status nullifier-query-server --no-pager'
ssh root@<host-ip> 'curl -fsS http://127.0.0.1:3000/ready'
```

If you need a specific baseline release, install manually from
`server-setup.md` and set `TAG=<baseline-tag>`.

## Local Benchmark Matrix

Run from the local checkout:

```bash
cd vote-nullifier-pir
mise exec -- cargo build --release -p pir-test

export PIR_URL=http://<host-ip>:3000
export NF=./nullifiers.bin
export LABEL_PREFIX=do-batch-test-$(date -u +%Y%m%dT%H%M%SZ)
```

Latency:

```bash
./target/release/pir-test bench-server \
  --url "$PIR_URL" \
  --nullifiers "$NF" \
  --iterations 30 --warmup 3 \
  --batch-size 5 --mode parallel \
  --seed 42 \
  --label "$LABEL_PREFIX-k5-parallel" \
  --json-out "docs/baselines/$LABEL_PREFIX-k5-parallel.json"

./target/release/pir-test bench-server \
  --url "$PIR_URL" \
  --nullifiers "$NF" \
  --iterations 30 --warmup 3 \
  --batch-size 5 --mode batched \
  --seed 42 \
  --label "$LABEL_PREFIX-k5-batched" \
  --json-out "docs/baselines/$LABEL_PREFIX-k5-batched.json"
```

Throughput:

```bash
./target/release/pir-test load \
  --url "$PIR_URL" \
  --nullifiers "$NF" \
  --mode single \
  --concurrency 8 \
  --duration 120s \
  --warmup 15s \
  --seed 42 \
  --json-out "docs/baselines/$LABEL_PREFIX-load-single-c8.json"

./target/release/pir-test load \
  --url "$PIR_URL" \
  --nullifiers "$NF" \
  --mode batched \
  --batch-size 5 \
  --concurrency 8 \
  --duration 120s \
  --warmup 15s \
  --seed 42 \
  --json-out "docs/baselines/$LABEL_PREFIX-load-batched-k5-c8.json"
```

## Install K-Wide Build

Copy the K-wide `nf-server` binary to the same host, restart, and rerun the
exact same matrix with a new `LABEL_PREFIX`.

## Cleanup

Destroy the test host when done:

```bash
doctl compute droplet delete vote-nullifier-pir-batch-test
doctl compute volume delete pir-batch-test-data
```
