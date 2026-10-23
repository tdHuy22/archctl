import static hipstershop.AdServiceGrpc.*;

/** Nhập tĩnh kiểu wildcard; cận kiểu tham số không phải chỗ cài đặt. */
class Wild<S extends hipstershop.AdServiceGrpc.AdServiceImplBase> {
  void call(Object ch) {
    newFutureStub(ch);
  }

  <T extends com.acme.shop.v1.CartServiceGrpc.AsyncService> void bound(T t) {}
}
