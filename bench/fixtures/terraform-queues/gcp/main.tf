# Pub/Sub đẩy tới Cloud Run qua push subscription; subscription kéo (pull)
# không có bên nhận trong Terraform.
resource "google_pubsub_topic" "orders" {
  name = "orders"
}

resource "google_cloud_run_v2_service" "billing" {
  name     = "billing"
  location = "us-central1"
  template {
    containers {
      image = "us-docker.pkg.dev/cloudrun/container/hello"
    }
  }
}

resource "google_pubsub_subscription" "billing" {
  name  = "billing"
  topic = google_pubsub_topic.orders.id
  push_config {
    push_endpoint = google_cloud_run_v2_service.billing.uri
  }
}

resource "google_pubsub_subscription" "audit" {
  name  = "audit"
  topic = google_pubsub_topic.orders.id
}
