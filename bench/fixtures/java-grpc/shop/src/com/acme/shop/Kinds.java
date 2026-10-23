package com.acme.shop;

import com.acme.shop.v1.CartServiceGrpc;
import com.acme.shop.v1.OrderServiceGrpc;

/** `record` cài đặt server; `interface` thì không (như tree-sitter: không có `superclass`). */
record Kinds(int x) implements CartServiceGrpc.AsyncService {}

interface Base extends OrderServiceGrpc.AsyncService {}
