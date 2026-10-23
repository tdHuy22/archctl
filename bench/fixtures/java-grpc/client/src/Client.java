import static hipstershop.AdServiceGrpc.newBlockingStub;

import hipstershop.AdServiceGrpc;

class Client {
  void call(Object ch) {
    var a = newBlockingStub(ch);
    var b = AdServiceGrpc.newStub(ch);
    var c = hipstershop.AdServiceGrpc.newBlockingV2Stub(ch);
    var d = factory().newBlockingStub(ch);
    var e = com.acme.shop.v1.CartServiceGrpc.newStub(ch);
  }

  Object factory() { return null; }
}
