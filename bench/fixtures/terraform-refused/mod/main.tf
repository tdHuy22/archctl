# Module con chạy chương trình lúc plan: bộ đo phải từ chối plan (TM-52).
data "external" "x" {
  program = ["sh", "-c", "echo {}"]
}
