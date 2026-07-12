output "api_endpoint" {
  description = "The ingest API's direct endpoint (also reachable through the distribution)."
  value       = aws_apigatewayv2_api.ingest.api_endpoint
}

output "distribution_domain" {
  description = "The CloudFront domain serving snapshots (and fronting ingest)."
  value       = aws_cloudfront_distribution.market_data.domain_name
}

output "acm_validation_records" {
  description = "DNS records that must exist for the certificate to issue."
  value = [
    for option in aws_acm_certificate.market_data.domain_validation_options : {
      name  = option.resource_record_name
      type  = option.resource_record_type
      value = option.resource_record_value
    }
  ]
}

output "domain_dns_record" {
  description = "The DNS record that points the service domain at the distribution."
  value = {
    name  = var.domain_name
    type  = "CNAME"
    value = aws_cloudfront_distribution.market_data.domain_name
  }
}

output "observation_table" {
  description = "The DynamoDB observation table name."
  value       = aws_dynamodb_table.observations.name
}

output "snapshot_bucket" {
  description = "The snapshot bucket name."
  value       = aws_s3_bucket.snapshots.bucket
}
