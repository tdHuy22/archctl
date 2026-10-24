//! Import Java: `import`, tên đầy đủ trong mã, tên trần cùng package, phân
//! giải theo đường dẫn package và câu `package` của file đích.

use arch_core::check::{Report, UnresolvedReason, Via, check, check_files};
use std::path::PathBuf;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir().join(format!("archctl-java-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Repo(root)
    }

    fn write(&self, rel: &str, body: &str) -> &Self {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
    }

    fn doc(&self) -> arch_core::Document {
        let src = std::fs::read_to_string(self.0.join(arch_core::ARCH_FILE)).unwrap();
        let (doc, diags) = arch_core::load(&src);
        assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
        doc
    }

    fn report(&self) -> Report {
        check(&self.0, &self.doc(), &[])
    }

    fn partial(&self, files: &[&str]) -> Report {
        let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
        check_files(&self.0, &self.doc(), &files, true)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn edges(r: &Report) -> Vec<String> {
    r.violations
        .iter()
        .map(|v| format!("{:?} {} -> {} {}:{} {}", v.reason, v.from, v.to, v.file, v.line, v.spec))
        .collect()
}

const LAW: &str = "component Core at core/**\ncomponent App at app/**\nallow App -> App\nallow Core -> Core\n";
const UTIL: &str = "package com.acme.core;\npublic class Util {\n  public static class Inner {}\n  public static int helper() { return 1; }\n}\n";

/// Maven nhiều module: mọi cách viết import trỏ tới `core` đều là cạnh
/// App → Core, thiếu `allow` là vi phạm; có `allow` thì hết.
#[test]
fn import_giua_hai_module() {
    let r = Repo::new("modules");
    r.write("core/src/main/java/com/acme/core/Util.java", UTIL)
        .write(
            "app/src/main/java/com/acme/app/A.java",
            "package com.acme.app;\nimport com.acme.core.Util;\nimport java.util.List;\nclass A {}\n",
        )
        .write("app/src/main/java/com/acme/app/B.java", "package com.acme.app;\nimport com.acme.core.*;\nclass B {}\n")
        .write(
            "app/src/main/java/com/acme/app/C.java",
            "package com.acme.app;\nimport static com.acme.core.Util.helper;\nimport com.acme.core.Util.Inner;\nclass C {}\n",
        )
        .write(".arch/system.arch", LAW);
    let rep = r.report();
    assert_eq!(
        edges(&rep),
        [
            "NoAllow App -> Core app/src/main/java/com/acme/app/A.java:2 com.acme.core.Util",
            "NoAllow App -> Core app/src/main/java/com/acme/app/B.java:2 com.acme.core.*",
            "NoAllow App -> Core app/src/main/java/com/acme/app/C.java:2 com.acme.core.Util.helper",
            "NoAllow App -> Core app/src/main/java/com/acme/app/C.java:3 com.acme.core.Util.Inner",
        ]
    );
    assert!(rep.unresolved.is_empty(), "JDK là gói ngoài, không phải chưa phân giải: {:?}", rep.unresolved);
    assert!(rep.violations.iter().all(|v| v.via == Via::Import));

    r.write(".arch/system.arch", &format!("{LAW}allow App -> Core\n"));
    assert!(r.report().violations.is_empty());
}

/// Không qua `import`: tên đầy đủ viết thẳng trong mã, và file khai cùng
/// package với `core` rồi dùng tên trần. Hai cách lách dễ nhất (TM-71).
#[test]
fn ten_day_du_va_package_tach_doi() {
    let r = Repo::new("refs");
    r.write("core/src/main/java/com/acme/core/Util.java", UTIL)
        .write(
            "app/src/main/java/com/acme/app/A.java",
            "package com.acme.app;\nclass A {\n  int x = com.acme.core.Util.helper();\n  String s = \"com.acme.core.Util\"; // com.acme.core.Util\n}\n",
        )
        .write("app/src/main/java/com/acme/core/Sneaky.java", "package com.acme.core;\nclass Sneaky {\n  int y = Util.helper();\n}\n")
        .write(".arch/system.arch", LAW);
    assert_eq!(
        edges(&r.report()),
        [
            "NoAllow App -> Core app/src/main/java/com/acme/app/A.java:3 com.acme.core.Util (không qua import)",
            "NoAllow App -> Core app/src/main/java/com/acme/core/Sneaky.java:3 com.acme.core.Util (không qua import)",
        ]
    );
    // Hook chỉ sửa một file vẫn thấy.
    assert_eq!(edges(&r.partial(&["app/src/main/java/com/acme/core/Sneaky.java"])).len(), 1);
}

/// Nhập riêng một lớp cùng tên che lớp cùng package (JLS 6.4.1), nên tên trần
/// lúc đó không trỏ sang `core`. Lớp do chính file khai cũng vậy.
#[test]
fn nhap_rieng_che_ten_cung_package() {
    let r = Repo::new("shadow");
    r.write("core/src/main/java/com/acme/core/Util.java", UTIL)
        .write(
            "app/src/main/java/com/acme/core/X.java",
            "package com.acme.core;\nimport org.other.Util;\nclass X { int y = Util.helper(); }\n",
        )
        .write("app/src/main/java/com/acme/core/Y.java", "package com.acme.core;\nclass Y { static class Util {} Util u; }\n")
        .write(".arch/system.arch", LAW);
    assert!(r.report().violations.is_empty(), "{:?}", edges(&r.report()));
}

/// Đường dẫn khớp đuôi mà câu `package` khác thì không phải lớp đó; gốc test
/// của module khác không nằm trên classpath; cùng lớp ở hai module thì báo.
#[test]
fn chon_dung_file() {
    let r = Repo::new("pick");
    r.write("core/src/main/java/com/acme/core/Util.java", UTIL)
        .write("core/src/test/java/com/acme/core/Util.java", UTIL)
        .write("lib/x/com/acme/core/Util.java", "package x.com.acme.core;\npublic class Util {}\n")
        .write("app/src/main/java/com/acme/app/A.java", "package com.acme.app;\nimport com.acme.core.Util;\nclass A {}\n")
        .write(".arch/system.arch", &format!("component Lib at lib/**\n{LAW}"));
    let rep = r.report();
    assert_eq!(edges(&rep), ["NoAllow App -> Core app/src/main/java/com/acme/app/A.java:2 com.acme.core.Util"]);
    assert!(rep.unresolved.is_empty(), "{:?}", rep.unresolved);

    // Chỉ có bản test (tiện ích test dùng chung): vẫn là cạnh.
    r.write("core/src/test/java/com/acme/core/Asserts.java", "package com.acme.core;\npublic class Asserts {}\n")
        .write("app/src/test/java/com/acme/app/T.java", "package com.acme.app;\nimport com.acme.core.Asserts;\nclass T {}\n");
    assert_eq!(edges(&r.report()).len(), 2, "{:?}", edges(&r.report()));
    std::fs::remove_file(r.0.join("app/src/test/java/com/acme/app/T.java")).unwrap();

    // Bản sao thứ hai ở module khác: classpath quyết, archctl không đoán.
    r.write("other/src/main/java/com/acme/core/Util.java", UTIL);
    let rep = r.report();
    assert!(rep.violations.is_empty(), "{:?}", edges(&rep));
    assert_eq!(rep.unresolved.len(), 1);
    assert_eq!(rep.unresolved[0].reason, UnresolvedReason::Missing);
    assert_eq!(rep.unresolved[0].spec, "com.acme.core.Util");
}

/// Gói ngoài khớp `external` theo tiền tố package.
#[test]
fn external_theo_tien_to_package() {
    let r = Repo::new("external");
    r.write(
        "app/src/A.java",
        "package com.acme.app;\nimport com.google.common.collect.ImmutableList;\nimport java.util.List;\nclass A { Object o = com.google.common.base.Strings.class; }\n",
    )
    .write(".arch/system.arch", "component App at app/**\nexternal Guava at com.google.common\ndeny App -> Guava\n");
    let rep = r.report();
    // Tên đầy đủ viết thẳng trong mã cũng là dùng thư viện đó.
    assert_eq!(
        edges(&rep),
        [
            "Denied App -> Guava app/src/A.java:2 com.google.common.collect.ImmutableList",
            "Denied App -> Guava app/src/A.java:4 com.google.common.base.Strings (không qua import)",
        ]
    );
}

/// Review opus 2026-10-24: các cách lách và báo nhầm đã chạy thật.
#[test]
fn cac_cach_lach_cua_review() {
    let r = Repo::new("bypass");
    r.write("core/src/main/java/com/acme/shared/Util.java", "package com.acme.shared;\npublic class Util { public static int helper() { return 1; } }\n")
        .write("core/src/main/java/com/acme/core/Util.java", UTIL)
        // Package tách đôi: app có lớp khác trong `com.acme.shared`, rồi dùng
        // `Util` của core qua `import com.acme.shared.*`.
        .write("app/src/main/java/com/acme/shared/Mine.java", "package com.acme.shared;\nclass Mine {}\n")
        .write("app/src/main/java/com/acme/app/W.java", "package com.acme.app;\nimport com.acme.shared.*;\nclass W { int x = Util.helper(); }\n")
        // Lớp lồng ở lớp khác không che tên ở lớp này.
        .write(
            "app/src/main/java/com/acme/core/Sneaky.java",
            "package com.acme.core;\nclass Sneaky { int y = Util.helper(); }\nclass Other { static class Util {} }\n",
        )
        // `0xE+…`, escape unicode trong tên.
        .write("app/src/main/java/com/acme/app/H.java", "package com.acme.app;\nclass H { int x = 0xE+com.acme.core.Util.helper(); }\n")
        .write("app/src/main/java/com/acme/app/U.java", "package com.acme.app;\nimport \\u0063om.acme.core.Util;\nclass U {}\n")
        .write(".arch/system.arch", LAW);
    let mut got = edges(&r.report());
    got.sort();
    assert_eq!(
        got,
        [
            "NoAllow App -> Core app/src/main/java/com/acme/app/H.java:2 com.acme.core.Util (không qua import)",
            "NoAllow App -> Core app/src/main/java/com/acme/app/U.java:2 com.acme.core.Util",
            "NoAllow App -> Core app/src/main/java/com/acme/app/W.java:3 com.acme.shared.Util (không qua import)",
            "NoAllow App -> Core app/src/main/java/com/acme/core/Sneaky.java:2 com.acme.core.Util (không qua import)",
        ]
    );
}

/// Bản sao do build sinh (bị `.gitignore` bỏ) không làm import thành "có ở
/// nhiều module"; file đích có câu `package` sau 64 KB vẫn được nhận; chạm
/// trần số tham chiếu thì báo, không bỏ im lặng.
#[test]
fn chi_muc_trung_check_va_tran() {
    let r = Repo::new("index");
    r.write(".gitignore", "build/\n")
        .write("core/src/main/java/com/acme/core/Util.java", &format!("/*{}*/\n{UTIL}", "x".repeat(70_000)))
        .write("core/build/generated/com/acme/core/Util.java", UTIL)
        .write("app/src/main/java/com/acme/app/A.java", "package com.acme.app;\nimport com.acme.core.Util;\nclass A {}\n")
        .write(".arch/system.arch", LAW);
    let rep = r.report();
    assert_eq!(edges(&rep), ["NoAllow App -> Core app/src/main/java/com/acme/app/A.java:2 com.acme.core.Util"]);
    assert!(rep.unresolved.is_empty(), "{:?}", rep.unresolved);

    let names: String = (0..70_000).map(|i| format!("V{i} v{i};\n")).collect();
    r.write("app/src/main/java/com/acme/app/B.java", &format!("package com.acme.app;\nclass B {{\n{names}}}\n"));
    let rep = r.report();
    assert!(rep.unresolved.iter().any(|u| u.file.ends_with("B.java") && u.spec.contains("quá nhiều")), "{:?}", rep.unresolved);
}

/// `init --from-code` thấy cạnh chỉ đi qua tham chiếu, nên luật sinh ra không
/// tự vi phạm; luật `at …/*.java` khớp được đích của `import p.*`.
#[test]
fn init_thay_tham_chieu_va_import_sao() {
    let r = Repo::new("init");
    r.write("core/src/main/java/com/acme/core/Util.java", UTIL)
        .write("lib/src/main/java/com/acme/lib/L.java", "package com.acme.lib;\nclass L { int x = com.acme.core.Util.helper(); }\n")
        .write("app/src/main/java/com/acme/app/A.java", "package com.acme.app;\nimport com.acme.core.*;\nclass A {}\n")
        .write(".arch/system.arch", "component Core at core/**/*.java\ncomponent App at app/**\ncomponent Lib at lib/**\nallow App -> App\nallow Lib -> Lib\n");
    assert_eq!(
        edges(&r.report()),
        [
            "NoAllow App -> Core app/src/main/java/com/acme/app/A.java:2 com.acme.core.*",
            "NoAllow Lib -> Core lib/src/main/java/com/acme/lib/L.java:2 com.acme.core.Util (không qua import)",
        ]
    );
    let found = arch_core::discover::discover(&r.0, &[]);
    let mut seen: Vec<(String, String)> = found.edges.clone();
    seen.sort();
    assert!(seen.contains(&("Lib".to_string(), "Core".to_string())), "{seen:?}");
}


/// Tham chiếu tới lớp có ở hai module được báo như import, không bỏ im lặng;
/// hai bản cùng một component (biến thể `src/debug`, `src/release` của
/// Android) thì vẫn là một đích rõ (review sonnet 2026-10-24).
#[test]
fn nhap_nhang_bao_ra_cung_component_thi_khong() {
    let r = Repo::new("ambiguous");
    r.write("core/src/main/java/com/acme/core/Util.java", UTIL)
        .write("other/src/main/java/com/acme/core/Util.java", UTIL)
        .write("app/src/main/java/com/acme/app/R.java", "package com.acme.app;\nclass R { int a = com.acme.core.Util.helper(); }\n")
        .write(".arch/system.arch", &format!("component Other at other/**\n{LAW}"));
    let rep = r.report();
    assert!(rep.violations.is_empty(), "{:?}", edges(&rep));
    assert_eq!(rep.unresolved.len(), 1, "{:?}", rep.unresolved);
    assert_eq!(rep.unresolved[0].spec, "com.acme.core.Util");

    let r = Repo::new("flavors");
    r.write("core/src/debug/java/com/acme/core/Util.java", UTIL)
        .write("core/src/release/java/com/acme/core/Util.java", UTIL)
        .write("app/src/main/java/com/acme/app/A.java", "package com.acme.app;\nimport com.acme.core.Util;\nclass A {}\n")
        .write(".arch/system.arch", LAW);
    let rep = r.report();
    assert_eq!(edges(&rep), ["NoAllow App -> Core app/src/main/java/com/acme/app/A.java:2 com.acme.core.Util"]);
    assert!(rep.unresolved.is_empty(), "{:?}", rep.unresolved);
}
