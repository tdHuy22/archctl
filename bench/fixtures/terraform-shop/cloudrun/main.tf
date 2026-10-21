# Hai service Cloud Run gọi nhau, theo mẫu run/interservice của Google.
resource "google_cloud_run_v2_service" "public" {
  name     = "public-service"
  location = "us-central1"
  template {
    containers {
      image = "us-docker.pkg.dev/cloudrun/container/hello"
      env {
        name  = "URL"
        value = google_cloud_run_v2_service.private.uri
      }
    }
  }
}

resource "google_cloud_run_v2_service" "private" {
  name     = "private-service"
  location = "us-central1"
  template {
    containers {
      image = "us-docker.pkg.dev/cloudrun/container/hello"
    }
  }
}

resource "google_cloud_run_v2_service_iam_member" "public" {
  location = google_cloud_run_v2_service.public.location
  name     = google_cloud_run_v2_service.public.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}
