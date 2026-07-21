# Snapshot distribution: a private S3 bucket readable only by CloudFront
# (origin access control), with the published JSON Schemas served from the
# same domain their $id URLs point at. The custom domain attaches in a
# second apply once the ACM certificate validates; see var.domain_ready.

resource "aws_s3_bucket" "snapshots" {
  bucket = var.snapshot_bucket_name

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_versioning" "snapshots" {
  bucket = aws_s3_bucket.snapshots.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_public_access_block" "snapshots" {
  bucket = aws_s3_bucket.snapshots.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

data "aws_iam_policy_document" "snapshots_cloudfront_read" {
  statement {
    sid       = "CloudFrontRead"
    actions   = ["s3:GetObject"]
    resources = ["${aws_s3_bucket.snapshots.arn}/*"]

    principals {
      type        = "Service"
      identifiers = ["cloudfront.amazonaws.com"]
    }

    condition {
      test     = "StringEquals"
      variable = "AWS:SourceArn"
      values   = [aws_cloudfront_distribution.market_data.arn]
    }
  }
}

resource "aws_s3_bucket_policy" "snapshots" {
  bucket = aws_s3_bucket.snapshots.id
  policy = data.aws_iam_policy_document.snapshots_cloudfront_read.json
}

# The app's webview reads the snapshot cross-origin, and its conditional
# refresh sends If-None-Match, which is not a CORS-safelisted header, so
# the browser preflights the GET. S3 answers both once the bucket carries
# a CORS configuration and CloudFront forwards the CORS request headers
# (see the default cache behaviour). ETag is exposed so the client can
# replay it as If-None-Match. The data is public; the wildcard origin is
# deliberate.
resource "aws_s3_bucket_cors_configuration" "snapshots" {
  bucket = aws_s3_bucket.snapshots.id

  cors_rule {
    allowed_origins = ["*"]
    allowed_methods = ["GET", "HEAD"]
    allowed_headers = ["*"]
    expose_headers  = ["ETag"]
    max_age_seconds = 3600
  }
}

# The contract schemas are published at the URLs their $id fields declare.
resource "aws_s3_object" "schemas" {
  for_each = fileset("${path.module}/../../schemas", "*.schema.json")

  bucket        = aws_s3_bucket.snapshots.id
  key           = "schemas/${each.value}"
  source        = "${path.module}/../../schemas/${each.value}"
  etag          = filemd5("${path.module}/../../schemas/${each.value}")
  content_type  = "application/json"
  cache_control = "public, max-age=3600"
}

# ── TLS ─────────────────────────────────────────────────────────────

resource "aws_acm_certificate" "market_data" {
  provider = aws.us_east_1

  domain_name       = var.domain_name
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

# ── CloudFront ──────────────────────────────────────────────────────

resource "aws_cloudfront_origin_access_control" "snapshots" {
  name                              = "entropiaorme-market-data-snapshots"
  origin_access_control_origin_type = "s3"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

locals {
  s3_origin_id  = "snapshots"
  api_origin_id = "ingest-api"

  # AWS-managed policies, by their fixed documented ids.
  cache_policy_caching_optimized       = "658327ea-f89d-4fab-a63d-7e88639e58f6"
  cache_policy_caching_disabled        = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad"
  origin_policy_all_viewer_except_host = "b689b0a8-53d0-40ab-baf2-68738e2966ac"
  origin_policy_cors_s3                = "88a5eaf4-2fd4-4709-b370-b4c650ea3fcf"
}

resource "aws_cloudfront_distribution" "market_data" {
  comment         = "EntropiaOrme market data (snapshots + ingest)"
  enabled         = true
  is_ipv6_enabled = true
  price_class     = "PriceClass_100"
  http_version    = "http2and3"
  aliases         = var.domain_ready ? [var.domain_name] : []

  origin {
    origin_id                = local.s3_origin_id
    domain_name              = aws_s3_bucket.snapshots.bucket_regional_domain_name
    origin_access_control_id = aws_cloudfront_origin_access_control.snapshots.id
  }

  origin {
    origin_id   = local.api_origin_id
    domain_name = replace(aws_apigatewayv2_api.ingest.api_endpoint, "https://", "")

    custom_origin_config {
      http_port              = 80
      https_port             = 443
      origin_protocol_policy = "https-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }

  # Snapshots and schemas: cached GETs against S3. Object Cache-Control
  # headers set by the aggregation govern freshness (immutable snapshot
  # objects, five-minute latest pointer). OPTIONS rides along so the
  # webview's CORS preflight (triggered by If-None-Match) reaches the
  # bucket's CORS responder; the CORS-S3Origin policy forwards Origin
  # and the Access-Control-Request-* headers. Caching stays safe with
  # Origin outside the cache key because the bucket answers wildcard
  # (the response never varies by origin).
  default_cache_behavior {
    target_origin_id         = local.s3_origin_id
    allowed_methods          = ["GET", "HEAD", "OPTIONS"]
    cached_methods           = ["GET", "HEAD", "OPTIONS"]
    cache_policy_id          = local.cache_policy_caching_optimized
    origin_request_policy_id = local.origin_policy_cors_s3
    viewer_protocol_policy   = "redirect-to-https"
    compress                 = true
  }

  # Submissions: pass through to the ingest API uncached.
  ordered_cache_behavior {
    path_pattern             = "/v1/submissions"
    target_origin_id         = local.api_origin_id
    allowed_methods          = ["GET", "HEAD", "OPTIONS", "PUT", "POST", "PATCH", "DELETE"]
    cached_methods           = ["GET", "HEAD"]
    cache_policy_id          = local.cache_policy_caching_disabled
    origin_request_policy_id = local.origin_policy_all_viewer_except_host
    viewer_protocol_policy   = "https-only"
  }

  restrictions {
    geo_restriction {
      restriction_type = "none"
    }
  }

  dynamic "viewer_certificate" {
    for_each = var.domain_ready ? [1] : []
    content {
      acm_certificate_arn      = aws_acm_certificate.market_data.arn
      ssl_support_method       = "sni-only"
      minimum_protocol_version = "TLSv1.2_2021"
    }
  }

  dynamic "viewer_certificate" {
    for_each = var.domain_ready ? [] : [1]
    content {
      cloudfront_default_certificate = true
    }
  }
}
