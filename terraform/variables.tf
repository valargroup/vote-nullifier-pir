variable "do_token" {
  description = "DigitalOcean API token"
  type        = string
  sensitive   = true
}

variable "cf_api_token" {
  description = "Cloudflare API token with DNS permissions"
  type        = string
  sensitive   = true
}

variable "cf_zone_id" {
  description = "Cloudflare zone ID for the domain"
  type        = string
}

variable "domain" {
  description = "Base domain (e.g. example.com). Used for pir-primary.<domain>, pir-backup.<domain>, pir.<domain>."
  type        = string
}

variable "ssh_key_fingerprints" {
  description = "List of DigitalOcean SSH key fingerprints for admin access"
  type        = list(string)
}

variable "region" {
  description = "DigitalOcean region"
  type        = string
  default     = "fra1"
}

variable "vpc_name" {
  description = "Name of the existing DigitalOcean VPC to join (managed by vote-sdk terraform)"
  type        = string
  default     = "vote-sdk-vpc"
}

# -----------------------------------------------------------------------------
# PIR hosts
# -----------------------------------------------------------------------------

variable "pir_primary_size" {
  description = "Droplet size slug for the PIR primary host (needs AVX-512 — Premium Intel)"
  type        = string
  default     = "g-8vcpu-32gb-intel"
}

variable "pir_backup_size" {
  description = "Droplet size slug for the PIR backup host (needs AVX-512 — Premium Intel)"
  type        = string
  default     = "m-4vcpu-32gb-intel"
}

variable "pir_volume_size" {
  description = "Size in GB for each PIR data block volume"
  type        = number
  default     = 100
}

variable "pir_release_tag" {
  description = "vote-nullifier-pir GitHub release tag for the nf-server binary"
  type        = string
  default     = "latest"
}

variable "pir_snapshot_url" {
  description = "Base URL of the DO Spaces bucket hosting nullifier snapshots"
  type        = string
  default     = "https://vote.fra1.digitaloceanspaces.com"
}

variable "pir_resync_on_calendar" {
  description = "systemd OnCalendar= spec for periodic snapshot re-pull on PIR hosts"
  type        = string
  default     = "*-*-* 03:00:00"
}
