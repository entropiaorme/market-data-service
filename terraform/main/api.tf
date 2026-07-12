# The ingest edge: an HTTP API fronting the ingest function, throttled at
# the gateway so abuse hits a limiter before it hits a bill. The API is
# also wired behind CloudFront (see distribution.tf) so clients use one
# domain for both submitting and fetching.

resource "aws_apigatewayv2_api" "ingest" {
  name          = "entropiaorme-market-data"
  protocol_type = "HTTP"
}

resource "aws_apigatewayv2_integration" "ingest" {
  api_id                 = aws_apigatewayv2_api.ingest.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.ingest.invoke_arn
  payload_format_version = "2.0"
}

resource "aws_apigatewayv2_route" "submissions" {
  api_id    = aws_apigatewayv2_api.ingest.id
  route_key = "POST /v1/submissions"
  target    = "integrations/${aws_apigatewayv2_integration.ingest.id}"
}

resource "aws_cloudwatch_log_group" "api_access" {
  name              = "/aws/apigateway/entropiaorme-market-data"
  retention_in_days = 30
}

resource "aws_apigatewayv2_stage" "default" {
  api_id      = aws_apigatewayv2_api.ingest.id
  name        = "$default"
  auto_deploy = true

  # The weekly cadence is one request per contributor; single-digit
  # sustained throughput is already two orders of magnitude of headroom.
  default_route_settings {
    throttling_burst_limit = 10
    throttling_rate_limit  = 5
  }

  access_log_settings {
    destination_arn = aws_cloudwatch_log_group.api_access.arn
    format = jsonencode({
      requestId      = "$context.requestId"
      requestTime    = "$context.requestTime"
      httpMethod     = "$context.httpMethod"
      path           = "$context.path"
      status         = "$context.status"
      responseLength = "$context.responseLength"
      sourceIp       = "$context.identity.sourceIp"
      integrationErr = "$context.integrationErrorMessage"
    })
  }
}

resource "aws_lambda_permission" "api_invoke" {
  statement_id  = "AllowApiGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.ingest.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.ingest.execution_arn}/*/*"
}
