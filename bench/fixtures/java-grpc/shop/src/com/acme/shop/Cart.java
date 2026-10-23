package com.acme.shop;

import com.acme.shop.v1.CartServiceGrpc;
import com.acme.shop.v1.OrderServiceGrpc;
import java.util.Map;

/** java_package khác package `.proto`; dạng AsyncService của grpc-java mới. */
class Cart implements Runnable, CartServiceGrpc.AsyncService {
  Map<String, OrderServiceGrpc.AsyncService> handlers;
  char quote = '"';
  String s = "OrderServiceGrpc.newStub(ch)";

  public void run() {
    var order = OrderServiceGrpc
        .newFutureStub(channel());
    /* CartServiceGrpc.newStub(ch) trong chú thích khối */
  }

  Object channel() { return null; }
}
