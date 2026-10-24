package com.acme.app;

import com.acme.core.Util;
import com.acme.core.Util.Inner;
import static com.acme.core.Util.*;

public class Page {
  Util u; Inner i;
  static Page make(Object... xs) { return new Page(); }
  Object o = List.of(Map.Entry.class, Helper.X);
}
class Tricky {
  int h = 0xE+com.acme.core.Util.n.hashCode();
  Object u = com.acme.core.Util.class;
  Object l = com.acme.core.helper.f();
  Object d = new com.acme.shared.$Gen();
  static class Nested { Helper x; }
  void m() { class Local {} Local a; }
  Local b;
  String s = "\\u0063om.fake.Ghost";
}
class Scopes {
  void f() { Util.a(); class Util {} Util.b(); }
  // dòng giấu \u000a Object hidden = com.acme.core.Util.class;
}
