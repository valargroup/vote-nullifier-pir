# -----------------------------------------------------------------------------
# DNS records (per-host, unproxied — Caddy needs ACME HTTP-01)
# -----------------------------------------------------------------------------

resource "cloudflare_record" "pir_primary" {
  zone_id = var.cf_zone_id
  name    = "pir-primary"
  content = digitalocean_droplet.pir_primary.ipv4_address
  type    = "A"
  ttl     = 1
  proxied = false
}

resource "cloudflare_record" "pir_backup" {
  zone_id = var.cf_zone_id
  name    = "pir-backup"
  content = digitalocean_droplet.pir_backup.ipv4_address
  type    = "A"
  ttl     = 1
  proxied = false
}

# -----------------------------------------------------------------------------
# Convenience record: pir.<domain> points to primary (manual failover)
# Replace with a Cloudflare Load Balancer when the LB subscription is active.
# -----------------------------------------------------------------------------

resource "cloudflare_record" "pir" {
  zone_id = var.cf_zone_id
  name    = "pir"
  content = digitalocean_droplet.pir_primary.ipv4_address
  type    = "A"
  ttl     = 1
  proxied = false
}
