//! Kiểm tra mô hình sau khi đọc: tham chiếu có tồn tại, tên có trùng, và luật
//! nào đang không kiểm được. Phần cuối quan trọng ngang phần đầu — một luật
//! không có cách kiểm thì chỉ là lời hứa, và hệ thống phải nói ra điều đó.

use crate::diag::Diag;
use crate::model::*;
use std::collections::HashMap;

/// Các loại invariant mà hệ thống hiểu. `algo` nằm trong danh sách nhưng không
/// kiểm được bằng máy, nên luôn bị cảnh báo riêng.
const INVARIANT_KINDS: &[&str] = &["data", "state", "money", "time", "access", "algo"];

pub fn validate(doc: &Document) -> Vec<Diag> {
    let mut diags = Vec::new();
    check_unique_names(doc, &mut diags);
    check_references(doc, &mut diags);
    check_flows(doc, &mut diags);
    check_verify_targets(doc, &mut diags);
    check_verifiability(doc, &mut diags);
    check_services(doc, &mut diags);
    diags
}

/// Service có không gian tên riêng, tách khỏi component: `component Api` và
/// `service Api` là chuyện bình thường ở repo mỗi service một thư mục.
fn check_services(doc: &Document, diags: &mut Vec<Diag>) {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut sources: HashMap<String, usize> = HashMap::new();
    for s in &doc.services {
        if s.name == INTERNET {
            diags.push(
                Diag::error(s.meta.line, format!("`{INTERNET}` là tên dành riêng cho lối vào từ ngoài, không khai `service` cho nó được"))
                    .with_hint(format!("viết `link {INTERNET} -> <Service>` để cho service đó lộ ra ngoài")),
            );
        }
        if let Some(first) = seen.insert(s.name.as_str(), s.meta.line) {
            diags.push(Diag::error(
                s.meta.line,
                format!("service `{}` trùng tên đã khai báo ở dòng {first}", s.name),
            ));
        }
        for source in &s.sources {
            // `dockerfile:` đi cùng `k8s:` là image của workload, nên nhiều
            // workload được phép dựng từ cùng một Dockerfile.
            if s.on_k8s() && matches!(source, Source::Dockerfile(_)) {
                continue;
            }
            if let Some(first) = sources.insert(source.text(), s.meta.line) {
                diags.push(
                    Diag::error(
                        s.meta.line,
                        format!("`from {}` đã dùng cho service khai báo ở dòng {first}", source.text()),
                    )
                    .with_hint("mỗi service trong compose hay workload Kubernetes ứng với đúng một `service`"),
                );
            }
        }
        let dockerfiles = s.sources.iter().filter(|x| matches!(x, Source::Dockerfile(_))).count();
        let compose = s.sources.iter().any(|x| matches!(x, Source::Compose { .. }));
        if dockerfiles > 1 || (dockerfiles == 1 && compose && !s.on_k8s()) {
            diags.push(
                Diag::error(s.meta.line, format!("service `{}`: `dockerfile:` thừa", s.name)).with_hint(
                    "compose đã nói service dựng từ đâu; `dockerfile:` chỉ đi một mình, \
                     hay cùng `k8s:` để nói image của workload dựng từ đâu",
                ),
            );
        }
        for name in &s.runs {
            match doc.component(name) {
                None => diags.push(
                    Diag::error(
                        s.meta.line,
                        format!("service `{}` chạy component `{name}` chưa khai báo", s.name),
                    )
                    .with_hint("`runs` liệt kê component, không phải service"),
                ),
                Some(c) if c.external => diags.push(
                    Diag::error(
                        s.meta.line,
                        format!("`runs {name}`: `{name}` là external, không có mã nào để chạy"),
                    )
                    .with_hint("thư viện ngoài đi theo component dùng nó, không cần liệt kê"),
                ),
                Some(_) => {}
            }
        }
    }

    for link in &doc.links {
        if link.to.iter().any(|t| t == INTERNET) {
            diags.push(
                Diag::error(link.meta.line, format!("`{INTERNET}` chỉ đứng ở đầu `link`: nó là lối vào từ ngoài"))
                    .with_hint("gọi ra dịch vụ ngoài thì khai `service` cho dịch vụ đó"),
            );
        }
        for name in std::iter::once(&link.from).chain(&link.to) {
            if name == INTERNET {
                continue;
            }
            if doc.service(name).is_none() {
                diags.push(
                    Diag::error(link.meta.line, format!("`link` trỏ tới service `{name}` chưa khai báo"))
                        .with_hint("`link` nối hai service; quan hệ giữa component thì dùng `allow`"),
                );
            }
        }
        if link.to.contains(&link.from) {
            diags.push(Diag::warning(
                link.meta.line,
                format!("`{}` nối tới chính nó, luật này vô nghĩa", link.from),
            ));
        }
        // Cạnh mạng chỉ thấy được trong cấu hình. Service không có `from` thì
        // không có cấu hình nào để đối chiếu, và `link` từ nó không bao giờ
        // kiểm được.
        if let Some(s) = doc.service(&link.from)
            && s.sources.is_empty()
            && link.meta.status.is_enforced()
        {
            diags.push(
                Diag::warning(
                    link.meta.line,
                    format!("service `{}` không có `from`, cạnh mạng của nó không kiểm được", s.name),
                )
                .with_hint(format!("thêm `from compose:<tên>` cho service {}", s.name)),
            );
        }
    }
}

/// Nhất quán bên trong `flow` và `boundary`. Những lỗi này bắt được ngay khi
/// đọc file, không cần nhìn mã.
fn check_flows(doc: &Document, diags: &mut Vec<Diag>) {
    for flow in &doc.flows {
        let declared: Vec<String> = flow
            .lines
            .iter()
            .filter(|l| !matches!(l, crate::model::FlowLine::OnFail { .. }))
            .flat_map(crate::model::flow_steps)
            .collect();

        for line in &flow.lines {
            let crate::model::FlowLine::OnFail { step, .. } = line else { continue };
            if !declared.contains(step) {
                diags.push(
                    Diag::error(
                        flow.meta.line,
                        format!("`on_fail({step})` nhưng `{step}` không phải bước của flow `{}`", flow.name),
                    )
                    .with_hint("bù trừ phải gắn vào một bước có thật trong luồng"),
                );
            }
        }
    }

    for boundary in &doc.boundaries {
        for field in boundary.fields.iter().filter(|f| f.key == "retry") {
            let text = field.values.join(" ");
            let Some(target) = text.split(':').next().map(str::trim) else { continue };
            let Some((component, capability)) = target.split_once('.') else { continue };
            if !doc
                .contracts
                .iter()
                .any(|c| c.component == component && c.capability == capability)
            {
                diags.push(
                    Diag::warning(
                        field.line,
                        format!("`retry {target}` nhưng không có contract nào tên đó"),
                    )
                    .with_hint("thử lại chỉ an toàn khi năng lực đó được khai báo là idempotent"),
                );
            }
        }
    }
}

/// `allow` và `deny` trỏ tới cả component lẫn store, nên hai loại dùng chung
/// một không gian tên.
fn check_unique_names(doc: &Document, diags: &mut Vec<Diag>) {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let entries = doc
        .components
        .iter()
        .map(|c| (c.name.as_str(), c.meta.line, "component"))
        .chain(doc.stores.iter().map(|s| (s.name.as_str(), s.meta.line, "store")))
        .chain(doc.boundaries.iter().map(|b| (b.name.as_str(), b.meta.line, "boundary")))
        .chain(doc.flows.iter().map(|f| (f.name.as_str(), f.meta.line, "flow")))
        .chain(doc.views.iter().map(|v| (v.name.as_str(), v.meta.line, "view")))
        .chain(doc.apis.iter().map(|a| (a.name.as_str(), a.meta.line, "api")))
        .chain(doc.verifies.iter().map(|v| (v.name.as_str(), v.meta.line, "cách kiểm")));

    for (name, line, what) in entries {
        if let Some(first) = seen.insert(name, line) {
            diags.push(
                Diag::error(line, format!("{what} `{name}` trùng tên đã khai báo ở dòng {first}"))
                    .with_hint("component, store, api, boundary, flow và view dùng chung không gian tên"),
            );
        }
    }

    let mut contracts: HashMap<(&str, &str), usize> = HashMap::new();
    for c in &doc.contracts {
        let key = (c.component.as_str(), c.capability.as_str());
        if let Some(first) = contracts.insert(key, c.meta.line) {
            diags.push(Diag::error(
                c.meta.line,
                format!(
                    "contract `{}.{}` trùng khai báo ở dòng {first}",
                    c.component, c.capability
                ),
            ));
        }
    }
}

fn check_references(doc: &Document, diags: &mut Vec<Diag>) {
    for store in &doc.stores {
        if doc.component(&store.owned_by).is_none() {
            diags.push(
                Diag::error(
                    store.meta.line,
                    format!("store `{}` thuộc component `{}` chưa khai báo",
                        store.name, store.owned_by),
                )
                .with_hint(format!("thêm `component {}` hoặc sửa lại tên", store.owned_by)),
            );
        }
    }

    for rule in &doc.rules {
        let line = rule.meta.line;
        for target in std::iter::once(&rule.from).chain(&rule.to) {
            if let Target::Named(name) = target
                && !doc.knows(name)
            {
                diags.push(Diag::error(
                    line,
                    format!("quan hệ trỏ tới `{name}` nhưng chưa có component hay store nào tên đó"),
                ));
            }
        }
        for name in &rule.except {
            if !doc.knows(name) {
                diags.push(Diag::error(
                    line,
                    format!("`except {name}` trỏ tới tên chưa khai báo"),
                ));
            }
        }
        if let Target::Named(from) = &rule.from
            && rule.to.iter().any(|t| matches!(t, Target::Named(to) if to == from))
        {
            diags.push(Diag::warning(line, format!("`{from}` trỏ tới chính nó, luật này vô nghĩa")));
        }
    }

    for boundary in &doc.boundaries {
        for name in &boundary.contains {
            if doc.component(name).is_none() {
                diags.push(Diag::error(
                    boundary.meta.line,
                    format!("boundary `{}` chứa component `{name}` chưa khai báo",
                        boundary.name),
                ));
            }
        }
    }

    for contract in &doc.contracts {
        if doc.component(&contract.component).is_none() {
            diags.push(Diag::error(
                contract.meta.line,
                format!("contract thuộc component `{}` chưa khai báo", contract.component),
            ));
        }
    }

    for view in &doc.views {
        for name in &view.components {
            if doc.component(name).is_none() {
                diags.push(Diag::error(
                    view.meta.line,
                    format!("view `{}` nhắc tới component `{name}` chưa khai báo", view.name),
                ));
            }
        }
    }

    for inv in &doc.invariants {
        if let Some(scope) = &inv.scope
            && scope != "system"
            && doc.component(scope).is_none()
            && doc.boundary(scope).is_none()
        {
            diags.push(
                Diag::error(inv.meta.line, format!("phạm vi `[{scope}]` không tồn tại"))
                    .with_hint("phạm vi là tên component, tên boundary, hoặc `system`"),
            );
        }
        if !INVARIANT_KINDS.contains(&inv.kind.as_str()) {
            diags.push(
                Diag::error(inv.meta.line, format!("loại invariant không nhận ra: `{}`", inv.kind))
                    .with_hint(format!("loại hợp lệ: {}", INVARIANT_KINDS.join(", "))),
            );
        }
    }
}

/// `verify x` phải trỏ tới một cách kiểm có thật, nếu không thì cái tên đó
/// không dẫn tới đâu cả — đúng tình trạng trước khi có khai báo `verify`.
fn check_verify_targets(doc: &Document, diags: &mut Vec<Diag>) {
    for (name, used_by, line) in doc.verify_uses() {
        if doc.verify(&name).is_none() {
            diags.push(
                Diag::warning(line, format!("`verify {name}` trong {used_by} chưa khai báo"))
                    .with_hint(format!("thêm: verify {name} runs \"<lệnh>\"")),
            );
        }
    }
    // Không có cảnh báo "cách kiểm này không ai dùng". Nó từng có, và nó sai:
    // `archctl verify` chạy mọi cách kiểm đang có hiệu lực, và `check` đòi
    // biên nhận còn hiệu lực cho từng cái — kể cả cái không invariant nào trỏ
    // tới. Đòi mỗi `verify` phải gắn vào một luật chỉ làm người ta bịa ra luật
    // để gắn, trong khi "test đạt" hay "không dependency ngoài std" là thứ
    // đáng kiểm thật mà không phải quan hệ giữa hai component nào.
}

fn check_verifiability(doc: &Document, diags: &mut Vec<Diag>) {
    for c in &doc.components {
        if c.paths.is_empty() && !c.external {
            diags.push(
                Diag::warning(
                    c.meta.line,
                    format!("component `{}` không có `at`, bộ kiểm chứng sẽ bỏ qua", c.name),
                )
                .with_hint(format!("thêm `at <glob>`, hoặc đổi thành `external {}`", c.name)),
            );
        }
    }

    for c in &doc.contracts {
        if !has_field(&c.fields, "verify") && c.meta.status.is_enforced() {
            diags.push(Diag::warning(
                c.meta.line,
                format!("contract `{}.{}` không có verify, đây chỉ là lời hứa",
                    c.component, c.capability),
            ));
        }
    }

    for b in &doc.boundaries {
        if !has_field(&b.fields, "verify") && b.meta.status.is_enforced() {
            diags.push(Diag::warning(
                b.meta.line,
                format!("boundary `{}` không có verify, đây chỉ là lời hứa", b.name),
            ));
        }
    }

    for inv in &doc.invariants {
        // Có cách kiểm **đã khai báo** thì hết là lời hứa: `archctl verify`
        // chạy lệnh thật và `check` đòi biên nhận còn hiệu lực. Điều đó đúng
        // cả với loại `algo` — chính nó là đường ra cho những gì phân tích
        // tĩnh không làm được, nên gọi một `algo` đã có cách kiểm là "chỉ là
        // ghi chú" thì sai.
        //
        // Phải là đã khai báo, chứ không chỉ có tên: `verify benchmark` trỏ
        // tới một cách kiểm không tồn tại thì invariant đó vẫn đúng là ghi
        // chú, và thoát được cảnh báo chỉ bằng cách gõ thêm một cái tên là
        // một lỗ để lách.
        let names = inv.verify_names();
        if !names.is_empty() && names.iter().all(|n| doc.verify(n).is_some()) {
            continue;
        }
        if inv.kind == "algo" {
            diags.push(
                Diag::warning(
                    inv.meta.line,
                    "invariant loại `algo` không kiểm được bằng máy, chỉ là ghi chú",
                )
                .with_hint("gắn `verify <tên>` để có cái kiểm thật, hoặc đừng dựa vào nó như một ràng buộc"),
            );
        } else if inv.meta.status.is_enforced() {
            diags.push(Diag::warning(
                inv.meta.line,
                format!("invariant {} không có verify, đây chỉ là lời hứa", inv.kind),
            ));
        }
    }
}

fn has_field(fields: &[Field], key: &str) -> bool {
    fields.iter().any(|f| f.key == key)
}
