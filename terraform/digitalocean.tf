# -----------------------------------------------------------------------------
# VPC (managed by vote-sdk — looked up by name)
# -----------------------------------------------------------------------------

data "digitalocean_vpc" "main" {
  name = var.vpc_name
}

# -----------------------------------------------------------------------------
# Project (managed by vote-sdk — looked up by name)
# -----------------------------------------------------------------------------

data "digitalocean_project" "vote_sdk" {
  name = "vote-sdk"
}

resource "digitalocean_project_resources" "pir" {
  project = data.digitalocean_project.vote_sdk.id
  resources = [
    digitalocean_droplet.pir_primary.urn,
    digitalocean_droplet.pir_backup.urn,
    digitalocean_volume.pir_primary_data.urn,
    digitalocean_volume.pir_backup_data.urn,
  ]
}

# -----------------------------------------------------------------------------
# Block volumes
# -----------------------------------------------------------------------------

resource "digitalocean_volume" "pir_primary_data" {
  region                  = var.region
  name                    = "chain-data-pir-primary"
  size                    = var.pir_volume_size
  initial_filesystem_type = "ext4"
  description             = "PIR data for vote-nullifier-pir-primary"
}

resource "digitalocean_volume" "pir_backup_data" {
  region                  = var.region
  name                    = "chain-data-pir-backup"
  size                    = var.pir_volume_size
  initial_filesystem_type = "ext4"
  description             = "PIR data for vote-nullifier-pir-backup"
}

# -----------------------------------------------------------------------------
# Droplet: vote-nullifier-pir-primary
# -----------------------------------------------------------------------------

resource "digitalocean_droplet" "pir_primary" {
  name     = "vote-nullifier-pir-primary"
  region   = var.region
  size     = var.pir_primary_size
  image    = "ubuntu-24-04-x64"
  vpc_uuid = data.digitalocean_vpc.main.id

  ssh_keys = var.ssh_key_fingerprints
  user_data = templatefile("${path.module}/cloud-init/pir.yaml", {
    role               = "primary"
    volume_name        = "chain-data-pir-primary"
    release_tag        = var.pir_release_tag
    snapshot_url       = var.pir_snapshot_url
    resync_on_calendar = var.pir_resync_on_calendar
    hostname           = "pir-primary.${var.domain}"
    caddyfile          = file("${path.module}/../deploy/pir.Caddyfile")
    systemd_unit       = file("${path.module}/../docs/nullifier-query-server.service")
  })

  volume_ids = [
    digitalocean_volume.pir_primary_data.id,
  ]

  tags = ["vote-sdk", "pir", "primary"]

  lifecycle {
    ignore_changes = [user_data]
  }
}

# -----------------------------------------------------------------------------
# Droplet: vote-nullifier-pir-backup
# -----------------------------------------------------------------------------

resource "digitalocean_droplet" "pir_backup" {
  name     = "vote-nullifier-pir-backup"
  region   = var.region
  size     = var.pir_backup_size
  image    = "ubuntu-24-04-x64"
  vpc_uuid = data.digitalocean_vpc.main.id

  ssh_keys = var.ssh_key_fingerprints
  user_data = templatefile("${path.module}/cloud-init/pir.yaml", {
    role               = "backup"
    volume_name        = "chain-data-pir-backup"
    release_tag        = var.pir_release_tag
    snapshot_url       = var.pir_snapshot_url
    resync_on_calendar = var.pir_resync_on_calendar
    hostname           = "pir-backup.${var.domain}"
    caddyfile          = file("${path.module}/../deploy/pir.Caddyfile")
    systemd_unit       = file("${path.module}/../docs/nullifier-query-server.service")
  })

  volume_ids = [
    digitalocean_volume.pir_backup_data.id,
  ]

  tags = ["vote-sdk", "pir", "backup"]

  lifecycle {
    ignore_changes = [user_data]
  }
}

# -----------------------------------------------------------------------------
# Firewall
# -----------------------------------------------------------------------------

resource "digitalocean_firewall" "pir" {
  name = "vote-pir-fw"
  droplet_ids = [
    digitalocean_droplet.pir_primary.id,
    digitalocean_droplet.pir_backup.id,
  ]

  inbound_rule {
    protocol         = "tcp"
    port_range       = "22"
    source_addresses = ["0.0.0.0/0", "::/0"]
  }

  inbound_rule {
    protocol         = "tcp"
    port_range       = "443"
    source_addresses = ["0.0.0.0/0", "::/0"]
  }

  inbound_rule {
    protocol         = "tcp"
    port_range       = "80"
    source_addresses = ["0.0.0.0/0", "::/0"]
  }

  outbound_rule {
    protocol              = "tcp"
    port_range            = "1-65535"
    destination_addresses = ["0.0.0.0/0", "::/0"]
  }

  outbound_rule {
    protocol              = "udp"
    port_range            = "1-65535"
    destination_addresses = ["0.0.0.0/0", "::/0"]
  }

  outbound_rule {
    protocol              = "icmp"
    destination_addresses = ["0.0.0.0/0", "::/0"]
  }
}
