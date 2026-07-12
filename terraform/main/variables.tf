variable "aws_region" {
  description = "The home region for the service (CloudFront is global; everything else lives here)."
  type        = string
  default     = "eu-north-1"
}

variable "domain_name" {
  description = "The public domain snapshots are served from."
  type        = string
  default     = "market-data.entropiaorme.com"
}

variable "domain_ready" {
  description = <<-EOT
    Whether the ACM certificate has been DNS-validated, so the distribution
    can carry the custom domain. Two-step by necessity: the first apply
    creates the certificate and outputs the validation record; once that
    record exists in DNS and the certificate reads ISSUED, a second apply
    with this set to true attaches the alias. Until then the distribution
    serves on its *.cloudfront.net domain.
  EOT
  type        = bool
  default     = false
}

variable "alert_email" {
  description = "Where operational alarms land."
  type        = string
}

variable "table_name" {
  description = "Name of the DynamoDB observation table."
  type        = string
  default     = "entropiaorme-market-data"
}

variable "snapshot_bucket_name" {
  description = "Globally-unique name for the snapshot bucket."
  type        = string
  default     = "entropiaorme-market-data-snapshots"
}

variable "aggregation_schedule" {
  description = "When the aggregation runs (UTC). Daily is deliberate headroom over the weekly ingest cadence."
  type        = string
  default     = "cron(30 3 * * ? *)"
}
