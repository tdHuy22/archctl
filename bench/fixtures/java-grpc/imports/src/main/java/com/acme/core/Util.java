// Bộ mẫu import Java: chữ giống câu import trong chuỗi, text block, chú thích.
package com.acme
    .core;

import java.util.*;
import static java.util.Objects
    .requireNonNull;
import com.acme /* tách */ . app . Page;

public class Util {
  public static class Inner {}
  @interface Marker {}
  record Pair(int a, int b) {}
  static final String S = "import com.fake.Nope; com.fake.Ghost.of()";
  static final String T = """
      import com.fake.Text;
      com.fake.Block.run();
      """;
  // import com.fake.Comment;  com.fake.Comment.x()
  /* com.fake.Block2.y() */
  long n = 42L + 0x1FL + 1_000L;
  double d = 1.5e-3D + .5F;
  char c = '"';
  Object o = com.acme
      .app.Page.make(Marker.class, Pair.class, Inner.class, Helper.VALUE);
  Object p = new java.util.ArrayList<com.acme.app.Page>();
  Object q = this.field.Name;
}
