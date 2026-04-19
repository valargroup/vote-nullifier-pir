output "pir_primary_ip" {
  description = "Public IPv4 address of the PIR primary Droplet"
  value       = digitalocean_droplet.pir_primary.ipv4_address
}

output "pir_backup_ip" {
  description = "Public IPv4 address of the PIR backup Droplet"
  value       = digitalocean_droplet.pir_backup.ipv4_address
}

output "pir_primary_url" {
  description = "Direct HTTPS URL for the PIR primary host"
  value       = "https://pir-primary.${var.domain}"
}

output "pir_backup_url" {
  description = "Direct HTTPS URL for the PIR backup host"
  value       = "https://pir-backup.${var.domain}"
}

output "pir_url" {
  description = "Public PIR URL (points to primary; use this in clients)"
  value       = "https://pir.${var.domain}"
}
