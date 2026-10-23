package hipstershop;

import io.grpc.Server;
import io.grpc.ServerBuilder;
import io.grpc.health.v1.HealthGrpc;

/** Server viết như Online Boutique: lớp lồng kế thừa tên đầy đủ. */
public final class AdService {
  private Server server;

  void start(int port) throws Exception {
    server = ServerBuilder.forPort(port)
        .addService(new AdServiceImpl())
        .build()
        .start();
  }

  private static class AdServiceImpl extends hipstershop.AdServiceGrpc.AdServiceImplBase {
    // extends hipstershop.AdServiceGrpc.FakeImplBase trong chú thích
    String doc = """
        AdServiceGrpc.newBlockingStub(channel) trong text block
        """;
  }

  private static class Health extends HealthGrpc.HealthImplBase {}
}
