# PIR APM sidecar

`pir-apm` is a standalone Rust service that scrapes the local `nf-server`,
computes five-minute endpoint rates and histogram quantiles, displays host and
snapshot health, and sends episode-based Slack alerts. It binds to loopback by
default and serves the dashboard at `/`, `/apm`, and `/apm/`; its own health
check is `/healthz`.

Tier1 latency is split into observed total time, request-body receive time, and
post-upload server processing time. The public dashboard shows observed total
and server processing latency, while body-receive timing remains available in
nf-server operator metrics. Only server processing is evaluated by the Tier1
high-latency alert.

## Build and install

The crate intentionally has its own empty `[workspace]` table, so it is not a
member of the repository workspace.

```sh
cargo build --release --manifest-path deploy/pir-apm/Cargo.toml
sudo install -m 0755 deploy/pir-apm/target/release/pir-apm /opt/nf-ingest/pir-apm
sudo install -m 0644 deploy/systemd/pir-apm.service /etc/systemd/system/pir-apm.service
sudo systemctl daemon-reload
sudo systemctl enable --now pir-apm
```

For a release built from inside `deploy/pir-apm`, the binary is at
`deploy/pir-apm/target/release/pir-apm`. The crate supports stable Rust 1.88 or
newer.

## Configuration

Put overrides in `/etc/default/pir-apm`:

```sh
PIR_APM_SCRAPE_URL=http://127.0.0.1:3000
PIR_APM_LISTEN=127.0.0.1:3002
PIR_APM_ENVIRONMENT=staging
PIR_APM_HOSTNAME=pir-primary
PIR_APM_DATA_DIR=/opt/nf-ingest/pir-data
PIR_APM_INTERVAL_SECONDS=15
# Obtain the value from the deployment secret store; do not commit it.
PIR_APM_SLACK_WEBHOOK_URL=...
```

`PIR_APM_SCRAPE_URL`, `PIR_APM_LISTEN`, and the interval have the values shown
above by default. The hostname defaults to the operating-system hostname.
Without `PIR_APM_SLACK_WEBHOOK_URL`, alert and recovery messages are written to
the journal and monitoring otherwise works normally. The webhook value is
never printed.

`PIR_APM_DASHBOARD` is a deployment/reverse-proxy setting and is deliberately
not read by the binary. Configure the proxy to route `/apm/` to
`http://127.0.0.1:3002`; keep the sidecar listener on loopback.

## Access

The dashboard has no authentication. Whatever the reverse proxy exposes is
world-readable, which is the intended configuration in
[`deploy/caddy/Caddyfile`](caddy/Caddyfile): `/apm*` proxies straight through,
the public `/metrics` route stays blocked, and `GET /tier1/row/*` is rejected
because that debug endpoint is not privacy-preserving. The page renders only the
aggregate data described under [Privacy](#privacy), so exposure is limited to
service health rather than anything about individual queries. To keep it
private instead, drop the `/apm*` handler from the proxy and reach the sidecar
over an SSH tunnel to `127.0.0.1:3002`.

Useful commands:

```sh
curl -fsS http://127.0.0.1:3002/healthz
sudo journalctl -u pir-apm
/opt/nf-ingest/pir-apm --send-test-alert
/opt/nf-ingest/pir-apm --force-alert high_latency
```

The forced high-latency command sends one synthetic alert, waits briefly, sends
its recovery, and exits.

## Privacy

The dashboard contains only aggregate metrics from the three allowlisted PIR
endpoints (`tier0`, `params_tier1`, and `tier1_query`), snapshot gauges, process
resident memory, and local host capacity. It does not collect or render request
bodies, nullifiers, IP addresses, headers, query identifiers, or other
per-user data. The Tier1 timing split uses aggregate histograms with the same
fixed endpoint label, and the public dashboard does not render the body-receive
distribution. HTML is rendered entirely by the sidecar; the browser re-polls
this dashboard every 15 seconds to refresh in place, and never calls the PIR
server or `/metrics`. With scripting disabled the page falls back to a plain
meta refresh.
