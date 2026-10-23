package com.acme.shop;

class Server {
  void start(io.grpc.ServerBuilder<?> b) {
    b.addService(new Cart());
  }
}
