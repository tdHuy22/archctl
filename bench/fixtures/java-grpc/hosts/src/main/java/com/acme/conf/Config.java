// Bộ mẫu tên máy trong chuỗi Java: khoá theo khai báo, phép gán, lời gọi đọc
// cấu hình, `? :`, annotation; chuỗi trong chú thích và text block không ở chỗ
// giá trị thì không tính.
package com.acme.conf;

import java.net.InetSocketAddress;

public class Config {
  static final String PAYMENT_URL = "http://payment:8080/api";
  private String host = "redis";
  String dbHost = "postgres";
  String[] hosts = new String[2];
  String label = "cart";
  String dsn = "jdbc:postgresql://db:5432/shop?user=app";
  // String commented = "http://ghost:80";
  /* "http://ghost2:80" */

  void init(String env) {
    this.host = "cache";
    hosts[0] = "queue";
    String addr = System.getenv().getOrDefault("REDIS_ADDR", "redis:6379");
    String mail = System.getProperty("mail.host", "smtp");
    String fallback = env != null ? env : "localhost:9000";
    String grpc = "checkout:5050" + "/x";
    String url = ("http://inventory:7000");
    String other = env + "http://notkey:81";
    for (String h : "a:80 b:81".split(" ")) {}
    new InetSocketAddress("ads", 9555);
    connect("SEARCH_HOST", "search");
    String t = """
        http://block:80
        """;
    log("""
        http://blockarg:80
        """);
    String esc = "http://escaped:8081\t";
  }

  @Endpoint(host = "metrics", port = 9090)
  void stats() {}

  void connect(String k, String v) {}
  void log(String s) {}
  @interface Endpoint { String host(); int port(); }
}
class Builders {
  void f(Object cf, java.util.Map<String, String> m, java.util.Properties props) {
    RedisClient.builder().host("redis").port(6379);
    cf.setHost("rabbit");
    new Jedis("redis3", 6379);
    props.setProperty("db.host", "pghost");
    m.put("host", "mapped");
    log.info("cart");
    new java.net.URL("http://catalog:3550/x");
    allowOrigin("http://browser:80");
  }
  @org.springframework.beans.factory.annotation.Value("${redis.host:redis-spring}")
  String springHost;
}
