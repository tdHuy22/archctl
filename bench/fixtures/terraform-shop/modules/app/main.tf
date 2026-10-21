variable "tag" {
  type = string
}

variable "subnets" {
  type = list(string)
}

variable "vpc_id" {
  type    = string
  default = "vpc-123"
}

locals {
  db_host = aws_db_instance.main.address
  image   = "acme/api:${var.tag}"
}

resource "aws_db_instance" "main" {
  identifier        = "shop"
  engine            = "postgres"
  instance_class    = "db.t3.micro"
  allocated_storage = 20
  username          = "shop"
  password          = "not-a-real-password"
}

resource "aws_sqs_queue" "jobs" {
  name = "jobs"
}

resource "aws_ecs_task_definition" "api" {
  family = "api"
  container_definitions = jsonencode([{
    name      = "api"
    image     = local.image
    essential = true
    command   = ["python", "-m", "api"]
    environment = [
      { name = "DB_HOST", value = local.db_host },
      { name = "QUEUE_URL", value = aws_sqs_queue.jobs.url },
      { name = "MODE", value = "fast" },
    ]
  }])
}

resource "aws_security_group" "open" {
  name   = "open"
  vpc_id = var.vpc_id
  ingress {
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_ecs_service" "api" {
  name            = "api"
  cluster         = "shop"
  task_definition = aws_ecs_task_definition.api.arn
  desired_count   = 1
  launch_type     = "FARGATE"
  load_balancer {
    target_group_arn = aws_lb_target_group.api.arn
    container_name   = "api"
    container_port   = 80
  }
  network_configuration {
    subnets          = var.subnets
    security_groups  = [aws_security_group.open.id]
    assign_public_ip = true
  }
}

resource "aws_lb" "public" {
  name     = "shop-public"
  internal = false
  subnets  = var.subnets
}

resource "aws_lb" "internal" {
  name     = "shop-internal"
  internal = true
  subnets  = var.subnets
}

resource "aws_lb_target_group" "api" {
  name        = "api"
  port        = 80
  protocol    = "HTTP"
  vpc_id      = var.vpc_id
  target_type = "ip"
}

resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.public.arn
  port              = 80
  protocol          = "HTTP"
  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.api.arn
  }
}

resource "aws_lambda_function" "worker" {
  function_name = "worker"
  role          = "arn:aws:iam::123456789012:role/worker"
  package_type  = "Image"
  image_uri     = "123456789012.dkr.ecr.us-east-1.amazonaws.com/worker:${var.tag}"
  environment {
    variables = {
      API_URL = "http://${aws_lb.internal.dns_name}"
      JOBS    = aws_sqs_queue.jobs.arn
    }
  }
}

resource "aws_lambda_function_url" "worker" {
  function_name      = aws_lambda_function.worker.function_name
  authorization_type = "NONE"
}

output "lb_dns" {
  value = aws_lb.public.dns_name
}
