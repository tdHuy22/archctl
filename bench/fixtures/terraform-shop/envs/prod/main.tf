# Module gốc: dựng ứng dụng từ module trong repo, có bộ cân bằng tải công khai.
module "app" {
  source  = "../../modules/app"
  tag     = "2.1"
  subnets = ["subnet-a", "subnet-b"]
}

resource "aws_ecs_task_definition" "report" {
  family = "report"
  # Heredoc JSON: biến môi trường trỏ bộ cân bằng tải của module con.
  container_definitions = <<EOF
[{"name": "report", "image": "acme/report:1", "essential": true,
  "environment": [{"name": "API_URL", "value": "http://${module.app.lb_dns}/v1"}]}]
EOF
}
