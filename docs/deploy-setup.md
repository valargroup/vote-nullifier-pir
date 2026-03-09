# Deploy setup for nf-server

The workflows in `.github/workflows/` handle building and deploying `nf-server`:

- **`deploy.yml`** — Builds on every push to `main` and deploys to a remote host via SSH.
- **`release.yml`** — Builds multi-platform binaries and publishes a GitHub Release on version tags.
- **`resync.yml`** — Manually triggers a nullifier resync (ingest + export + restart) on the remote host.

## 0. Moving cached data to the deploy directory

The service uses flat binary files for nullifier storage. To move them into the deploy directory (default `/opt/nullifier-ingest`):

```bash
# Create target directory (default matches workflow DEPLOY_PATH)
sudo mkdir -p /opt/nullifier-ingest

# Stop the service first if it is running
sudo systemctl stop nullifier-query-server || true

# Move data files
sudo mv /path/to/nullifiers.bin        /opt/nullifier-ingest/
sudo mv /path/to/nullifiers.checkpoint /opt/nullifier-ingest/
sudo mv /path/to/nullifiers.tree       /opt/nullifier-ingest/

# Ensure the deploy user can write (if deploy runs as a different user)
# sudo chown -R DEPLOY_USER:DEPLOY_USER /opt/nullifier-ingest

# Restart the service (see systemd unit below)
```

The unit file in `docs/nullifier-query-server.service` uses `/opt/nullifier-ingest` as the data directory by default.

## 1. GitHub repository secrets

In the repo: **Settings -> Secrets and variables -> Actions**, add:

| Secret              | Description |
|---------------------|-------------|
| `DEPLOY_HOST`       | Remote hostname or IP (e.g. `ingest.example.com` or `192.0.2.10`). |
| `DEPLOY_USER`       | SSH user on that host (e.g. `deploy` or `ubuntu`). |
| `SSH_PASSWORD`      | SSH password for that user. |

## 2. One-time setup on the remote host

### Directory and binaries

- Create the deploy directory. Default in the workflow is `DEPLOY_PATH: /opt/nullifier-ingest`.
- Ensure the SSH user can write to that directory.
- Either bootstrap the nullifier data (`make bootstrap`) or run an initial ingest.

### Query server (PIR HTTP API)

The `nf-server serve` subcommand starts the PIR HTTP server. It needs:

- **Nullifier data**: `nullifiers.bin` and `nullifiers.checkpoint` in the data directory.
- **PIR data**: Exported tier files in `pir-data/` (produced by `nf-server export`).
- **Port**: Configurable via `--port` (default 3000).

A systemd unit file is provided at `docs/nullifier-query-server.service`. Copy to `/etc/systemd/system/`:

```bash
sudo cp docs/nullifier-query-server.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable nullifier-query-server
sudo systemctl start nullifier-query-server
```

### Ingest (periodic sync)

Run `nf-server ingest` periodically (cron or systemd timer) to sync new nullifiers:

```bash
/opt/nullifier-ingest/nf-server ingest \
    --data-dir /opt/nullifier-ingest \
    --lwd-url https://zec.rocks:443
```

After ingest, re-export with `nf-server export` and restart the serve process, or use the `resync.yml` workflow to do all three steps remotely.

## 3. Changing deploy path or restart command

- **Deploy path**: Edit the `env.DEPLOY_PATH` in `.github/workflows/deploy.yml` (default `/opt/nullifier-ingest`).
- **Restart command**: Edit the "Install config and restart services" step in that workflow if you use a different service name.

## 4. Manual runs

Both `deploy.yml` and `resync.yml` support `workflow_dispatch`, so you can trigger them from **Actions -> Run workflow** without pushing to `main`.

## 5. Test locally

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
