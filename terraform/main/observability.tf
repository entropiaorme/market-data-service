# Operational alarms. Cost alarms live in the bootstrap stack (budget);
# these cover the pipeline's health: a failing function or a 5xx at the
# edge pages the maintainer. Thresholds are absolute and low because the
# healthy baseline is near-zero traffic: any error is signal.

resource "aws_sns_topic" "alerts" {
  name = "entropiaorme-market-data-alerts"
}

resource "aws_sns_topic_subscription" "alerts_email" {
  topic_arn = aws_sns_topic.alerts.arn
  protocol  = "email"
  endpoint  = var.alert_email
}

resource "aws_cloudwatch_metric_alarm" "ingest_errors" {
  alarm_name          = "entropiaorme-market-data-ingest-errors"
  alarm_description   = "The ingest function returned an error"
  namespace           = "AWS/Lambda"
  metric_name         = "Errors"
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.alerts.arn]

  dimensions = {
    FunctionName = aws_lambda_function.ingest.function_name
  }
}

resource "aws_cloudwatch_metric_alarm" "aggregate_errors" {
  alarm_name          = "entropiaorme-market-data-aggregate-errors"
  alarm_description   = "The aggregation function returned an error"
  namespace           = "AWS/Lambda"
  metric_name         = "Errors"
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.alerts.arn]

  dimensions = {
    FunctionName = aws_lambda_function.aggregate.function_name
  }
}

resource "aws_cloudwatch_metric_alarm" "api_5xx" {
  alarm_name          = "entropiaorme-market-data-api-5xx"
  alarm_description   = "The ingest API returned server errors"
  namespace           = "AWS/ApiGateway"
  metric_name         = "5xx"
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.alerts.arn]

  dimensions = {
    ApiId = aws_apigatewayv2_api.ingest.id
  }
}
