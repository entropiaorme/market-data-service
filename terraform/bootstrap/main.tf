# Bootstrap stack: the two things that must exist before anything else.
#
#   1. The Terraform state bucket the main stack backs onto (versioned,
#      encrypted, private). Terraform 1.10+ locks state natively via S3
#      lockfiles (`use_lockfile`), so no lock table is needed.
#   2. The cost guardrails: a monthly budget with actual and forecast
#      alerts, so spend surprises page the maintainer before a bill does.
#
# This stack deliberately keeps LOCAL state (the chicken-and-egg stack
# cannot back onto the bucket it creates); it manages only these
# low-churn resources and is applied from the runbook, not CI.

terraform {
  required_version = ">= 1.10"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.80"
    }
  }
}

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      Project   = "entropiaorme-market-data"
      ManagedBy = "terraform"
      Stack     = "bootstrap"
    }
  }
}

# ── Terraform state backend ─────────────────────────────────────────

resource "aws_s3_bucket" "tf_state" {
  bucket        = var.state_bucket_name
  force_destroy = true
}

resource "aws_s3_bucket_versioning" "tf_state" {
  bucket = aws_s3_bucket.tf_state.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "tf_state" {
  bucket = aws_s3_bucket.tf_state.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm     = "aws:kms"
      kms_master_key_id = null # SSE-KMS with the AWS-managed key
    }
    bucket_key_enabled = true
  }
}

resource "aws_s3_bucket_public_access_block" "tf_state" {
  bucket = aws_s3_bucket.tf_state.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# ── Cost guardrails ─────────────────────────────────────────────────

resource "aws_budgets_budget" "monthly" {
  name         = "entropiaorme-market-data-monthly"
  budget_type  = "COST"
  limit_amount = var.monthly_budget_usd
  limit_unit   = "USD"
  time_unit    = "MONTHLY"

  # Page early (actual spend crossing half the budget), again near the
  # ceiling, and on any forecast that the month will blow through it.
  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = 50
    threshold_type             = "PERCENTAGE"
    notification_type          = "ACTUAL"
    subscriber_email_addresses = [var.alert_email]
  }

  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = 90
    threshold_type             = "PERCENTAGE"
    notification_type          = "ACTUAL"
    subscriber_email_addresses = [var.alert_email]
  }

  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = 100
    threshold_type             = "PERCENTAGE"
    notification_type          = "FORECASTED"
    subscriber_email_addresses = [var.alert_email]
  }
}
