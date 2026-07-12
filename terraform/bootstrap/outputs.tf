output "state_bucket" {
  description = "The state bucket the main stack's S3 backend points at."
  value       = aws_s3_bucket.tf_state.bucket
}

output "aws_region" {
  description = "The home region, echoed for the backend configuration."
  value       = var.aws_region
}
