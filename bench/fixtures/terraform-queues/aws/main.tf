# Hàng đợi tới bên nhận: Lambda đọc SQS qua event source mapping, SNS đẩy
# vào SQS, Lambda đứng sau alias đọc Kinesis.
resource "aws_sqs_queue" "jobs" {
  name = "jobs"
}

resource "aws_sns_topic" "events" {
  name = "events"
}

resource "aws_kinesis_stream" "clicks" {
  name        = "clicks"
  shard_count = 1
}

resource "aws_lambda_function" "api" {
  function_name = "api"
  role          = "arn:aws:iam::123456789012:role/api"
  package_type  = "Image"
  image_uri     = "123456789012.dkr.ecr.us-east-1.amazonaws.com/api:1"
  environment {
    variables = {
      QUEUE_URL = aws_sqs_queue.jobs.url
      TOPIC_ARN = aws_sns_topic.events.arn
    }
  }
}

resource "aws_lambda_function" "worker" {
  function_name = "worker"
  role          = "arn:aws:iam::123456789012:role/worker"
  package_type  = "Image"
  image_uri     = "123456789012.dkr.ecr.us-east-1.amazonaws.com/worker:1"
}

resource "aws_lambda_function" "clicks" {
  function_name = "clicks"
  role          = "arn:aws:iam::123456789012:role/clicks"
  package_type  = "Image"
  image_uri     = "123456789012.dkr.ecr.us-east-1.amazonaws.com/clicks:1"
}

resource "aws_lambda_alias" "clicks_live" {
  name             = "live"
  function_name    = aws_lambda_function.clicks.function_name
  function_version = "1"
}

resource "aws_lambda_event_source_mapping" "jobs" {
  event_source_arn = aws_sqs_queue.jobs.arn
  function_name    = aws_lambda_function.worker.arn
}

resource "aws_lambda_event_source_mapping" "clicks" {
  event_source_arn  = aws_kinesis_stream.clicks.arn
  function_name     = aws_lambda_alias.clicks_live.arn
  starting_position = "LATEST"
}

resource "aws_sns_topic_subscription" "to_jobs" {
  topic_arn = aws_sns_topic.events.arn
  protocol  = "sqs"
  endpoint  = aws_sqs_queue.jobs.arn
}
