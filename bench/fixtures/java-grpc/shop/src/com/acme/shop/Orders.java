package com.acme.shop;

import com.acme.shop.v1.OrderServiceGrpc;

/** Lớp vô danh trong `addService(` cùng file. */
class Orders {
  void start(io.grpc.ServerBuilder<?> b) {
    b.addService(new OrderServiceGrpc.OrderServiceImplBase() {
      @Override public String toString() { return "OrderServiceGrpc.newStub(x)"; }
    });
  }
}
