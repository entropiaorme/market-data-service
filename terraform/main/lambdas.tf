# The two functions, built with cargo-lambda (see ../README.md) and zipped
# from the workspace's build output. Each gets its own role scoped to
# exactly the resources it touches; log groups are created here (with
# retention) rather than on first invoke, so the roles never need
# logs:CreateLogGroup.

locals {
  lambda_build_dir = "${path.module}/../../lambdas/target/lambda"
}

data "archive_file" "ingest" {
  type        = "zip"
  source_file = "${local.lambda_build_dir}/ingest/bootstrap"
  output_path = "${path.module}/build/ingest.zip"
}

data "archive_file" "aggregate" {
  type        = "zip"
  source_file = "${local.lambda_build_dir}/aggregate/bootstrap"
  output_path = "${path.module}/build/aggregate.zip"
}

# ── Ingest ──────────────────────────────────────────────────────────

resource "aws_cloudwatch_log_group" "ingest" {
  name              = "/aws/lambda/entropiaorme-market-data-ingest"
  retention_in_days = 30
}

data "aws_iam_policy_document" "lambda_assume" {
  statement {
    actions = ["sts:AssumeRole"]

    principals {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "ingest" {
  name               = "entropiaorme-market-data-ingest"
  assume_role_policy = data.aws_iam_policy_document.lambda_assume.json
}

data "aws_iam_policy_document" "ingest" {
  statement {
    sid = "ContributorLookupAndObservationWrites"
    actions = [
      "dynamodb:GetItem",
      "dynamodb:PutItem",
      "dynamodb:BatchWriteItem",
    ]
    resources = [aws_dynamodb_table.observations.arn]
  }

  statement {
    sid = "Logs"
    actions = [
      "logs:CreateLogStream",
      "logs:PutLogEvents",
    ]
    resources = ["${aws_cloudwatch_log_group.ingest.arn}:*"]
  }
}

resource "aws_iam_role_policy" "ingest" {
  name   = "least-privilege"
  role   = aws_iam_role.ingest.id
  policy = data.aws_iam_policy_document.ingest.json
}

resource "aws_lambda_function" "ingest" {
  function_name = "entropiaorme-market-data-ingest"
  description   = "Authenticates contributors, validates submissions, writes observations"

  filename         = data.archive_file.ingest.output_path
  source_code_hash = data.archive_file.ingest.output_base64sha256

  role          = aws_iam_role.ingest.arn
  runtime       = "provided.al2023"
  handler       = "bootstrap"
  architectures = ["arm64"]
  memory_size   = 128
  timeout       = 10

  environment {
    variables = {
      TABLE_NAME = aws_dynamodb_table.observations.name
    }
  }

  logging_config {
    log_format = "JSON"
    log_group  = aws_cloudwatch_log_group.ingest.name
  }
}

# ── Aggregation ─────────────────────────────────────────────────────

resource "aws_cloudwatch_log_group" "aggregate" {
  name              = "/aws/lambda/entropiaorme-market-data-aggregate"
  retention_in_days = 30
}

resource "aws_iam_role" "aggregate" {
  name               = "entropiaorme-market-data-aggregate"
  assume_role_policy = data.aws_iam_policy_document.lambda_assume.json
}

data "aws_iam_policy_document" "aggregate" {
  statement {
    sid       = "ReadObservations"
    actions   = ["dynamodb:Scan"]
    resources = [aws_dynamodb_table.observations.arn]
  }

  statement {
    sid       = "PublishSnapshots"
    actions   = ["s3:PutObject"]
    resources = ["${aws_s3_bucket.snapshots.arn}/v1/*"]
  }

  statement {
    sid       = "ReadCurrentLatestForUnchangedCheck"
    actions   = ["s3:GetObject"]
    resources = ["${aws_s3_bucket.snapshots.arn}/v1/latest.json"]
  }

  # Without ListBucket, a GetObject on a key that does not exist yet comes
  # back 403 instead of 404, and the first-ever run cannot tell "nothing
  # published" from a real failure. Scoped to the one key it may probe.
  statement {
    sid       = "DistinguishMissingLatestFromDenied"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.snapshots.arn]

    condition {
      test     = "StringEquals"
      variable = "s3:prefix"
      values   = ["v1/latest.json"]
    }
  }

  statement {
    sid = "Logs"
    actions = [
      "logs:CreateLogStream",
      "logs:PutLogEvents",
    ]
    resources = ["${aws_cloudwatch_log_group.aggregate.arn}:*"]
  }
}

resource "aws_iam_role_policy" "aggregate" {
  name   = "least-privilege"
  role   = aws_iam_role.aggregate.id
  policy = data.aws_iam_policy_document.aggregate.json
}

resource "aws_lambda_function" "aggregate" {
  function_name = "entropiaorme-market-data-aggregate"
  description   = "Folds stored observations into the published snapshot"

  filename         = data.archive_file.aggregate.output_path
  source_code_hash = data.archive_file.aggregate.output_base64sha256

  role          = aws_iam_role.aggregate.arn
  runtime       = "provided.al2023"
  handler       = "bootstrap"
  architectures = ["arm64"]
  memory_size   = 256
  timeout       = 60

  environment {
    variables = {
      TABLE_NAME      = aws_dynamodb_table.observations.name
      SNAPSHOT_BUCKET = aws_s3_bucket.snapshots.bucket
    }
  }

  logging_config {
    log_format = "JSON"
    log_group  = aws_cloudwatch_log_group.aggregate.name
  }
}

# ── Schedule ────────────────────────────────────────────────────────

resource "aws_cloudwatch_event_rule" "aggregation" {
  name                = "entropiaorme-market-data-aggregation"
  description         = "Runs the snapshot aggregation"
  schedule_expression = var.aggregation_schedule
}

resource "aws_cloudwatch_event_target" "aggregation" {
  rule = aws_cloudwatch_event_rule.aggregation.name
  arn  = aws_lambda_function.aggregate.arn
}

resource "aws_lambda_permission" "aggregation_schedule" {
  statement_id  = "AllowEventBridgeInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.aggregate.function_name
  principal     = "events.amazonaws.com"
  source_arn    = aws_cloudwatch_event_rule.aggregation.arn
}
