# Disable MPTCP on a Linux PIR server

For native Ubuntu/Debian hosts running `nf-server` behind Caddy. No binary
update is needed, and the Rust PIR backend can stay running.

## Apply

Run on each PIR host. Record the public `/root` response before starting.
Restart Caddy even if MPTCP is already disabled; a reload does not replace
existing listeners.

```bash
(
  set -e
  printf '%s\n' 'net.mptcp.enabled = 0' | sudo tee /etc/sysctl.d/90-vote-disable-mptcp.conf
  sudo sysctl -p /etc/sysctl.d/90-vote-disable-mptcp.conf
  test "$(sysctl -n net.mptcp.enabled)" = 0
  sudo systemctl restart caddy
  sudo systemctl is-active --quiet caddy nullifier-query-server
)
```

## Verify

Requires Python 3.9+ on Linux x86_64 or aarch64. Once Caddy is listening,
download the pinned verifier and check its checksum before running it:

```bash
(
  set -e
  MITIGATION_DIR=$(mktemp -d)
  cd "$MITIGATION_DIR"
  curl -fsSLo check-mptcp-listeners.py https://raw.githubusercontent.com/valargroup/vote-sdk/8afb0d77dd125755d5ab373f0eb8c8964b10b364/scripts/check-mptcp-listeners.py
  echo 'f4a8fbdae4d7822f24b5264559f6748f55f90dc50d7e307c0d0f5c29b975acfc  check-mptcp-listeners.py' | sha256sum -c -
  sudo python3 check-mptcp-listeners.py --require-process nf-server --require-process caddy
)
```

Require exit **0**, `mptcp_listeners: 0`, and no errors. An empty `ss -M -l`
result is insufficient. Confirm the public `/ready` returns `status: ok` and
`/root` matches the response recorded before the restart, including its height.
Check for conflicting sysctl settings that could re-enable MPTCP after reboot.

For failures, inspect `sudo journalctl -u caddy -u nullifier-query-server -n 100 --no-pager`.
Container deployments need separate network-namespace handling.
