//! Parse `--admin-listen` / `SVOTE_PIR_ADMIN_LISTEN` for the private admin HTTP listener.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

/// Where the admin HTTP server binds (`unix://…` or `tcp://…`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminBind {
    Unix(PathBuf),
    Tcp(SocketAddr),
}

/// Parse `unix:///path` or `tcp://HOST:PORT` (scheme required).
pub fn parse_admin_listen(spec: &str, allow_public_tcp: bool) -> Result<AdminBind> {
    let s = spec.trim();
    if s.is_empty() {
        bail!("admin listen spec is empty");
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("unix://") {
        #[cfg(not(unix))]
        bail!("unix:// admin listen is only supported on unix targets");
        #[cfg(unix)]
        {
            // `unix:///abs/path` → remainder is `/abs/path`; do not strip leading `/`.
            let path = PathBuf::from(s.get(7..).unwrap_or(""));
            if path.as_os_str().is_empty() {
                bail!("unix admin listen path is empty (expected unix:///path/to/socket)");
            }
            return Ok(AdminBind::Unix(path));
        }
    }
    if lower.starts_with("tcp://") {
        let rest = s
            .get(6..)
            .context("tcp:// admin listen missing host:port")?
            .trim();
        let addr: SocketAddr = rest
            .parse()
            .with_context(|| format!("invalid tcp admin listen address: {rest}"))?;
        if !allow_public_tcp && !addr.ip().is_loopback() {
            bail!(
                "admin tcp bind {addr} is not loopback; use loopback or pass \
                 --admin-listen-allow-public (dangerous: exposes admin API on the network)"
            );
        }
        return Ok(AdminBind::Tcp(addr));
    }
    bail!(
        "invalid admin listen spec {s:?} (expected unix:///path or tcp://HOST:PORT, scheme required)"
    );
}

/// Parse `SVOTE_PIR_ADMIN_ENDPOINT` for the snapshot CLI (same schemes as [`parse_admin_listen`]).
pub fn parse_admin_endpoint(spec: &str) -> Result<AdminBind> {
    // CLI talks to an already-bound server; allow any tcp address the operator chooses.
    parse_admin_listen(spec, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn parse_unix_absolute() {
        let b = parse_admin_listen("unix:///run/nf-server/admin.sock", false).unwrap();
        assert_eq!(
            b,
            AdminBind::Unix(PathBuf::from("/run/nf-server/admin.sock"))
        );
    }

    #[test]
    fn parse_tcp_loopback_ok() {
        let b = parse_admin_listen("tcp://127.0.0.1:3001", false).unwrap();
        assert_eq!(
            b,
            AdminBind::Tcp("127.0.0.1:3001".parse().unwrap())
        );
    }

    #[test]
    fn parse_tcp_non_loopback_rejected_without_flag() {
        let e = parse_admin_listen("tcp://0.0.0.0:3001", false).unwrap_err();
        assert!(
            e.to_string().contains("loopback"),
            "unexpected error: {e:#}"
        );
    }

    #[test]
    fn parse_tcp_non_loopback_allowed_with_flag() {
        let b = parse_admin_listen("tcp://0.0.0.0:3001", true).unwrap();
        assert_eq!(
            b,
            AdminBind::Tcp("0.0.0.0:3001".parse().unwrap())
        );
    }

    #[test]
    fn parse_empty_fails() {
        assert!(parse_admin_listen("", false).is_err());
    }
}
