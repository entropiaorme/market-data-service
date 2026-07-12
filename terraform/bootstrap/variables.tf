variable "aws_region" {
  description = "The home region for the service (CloudFront is global; everything else lives here)."
  type        = string
  default     = "eu-north-1"
}

variable "state_bucket_name" {
  description = "Globally-unique name for the Terraform state bucket."
  type        = string
}

variable "monthly_budget_usd" {
  description = "Monthly cost ceiling in USD; alerts fire at 50% and 90% actual and 100% forecast."
  type        = string
  default     = "5"
}

variable "alert_email" {
  description = "Where budget alerts land."
  type        = string
}
