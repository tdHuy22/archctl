//! Đọc lệnh khởi chạy của một container để biết nó chạy mã nào.
//!
//! Chỉ tách chữ, không bao giờ chạy (TM-11). Kết quả là gợi ý: module Python,
//! file script, script trong `package.json`, package Go. Bên gọi tự đổi
//! chúng thành file trong repo.

use super::dockerfile::shell_words;

/// Một thứ mà lệnh khởi chạy nạp.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Launch {
    /// `python -m a.b`, `uvicorn a.b:app`, `celery -A a.b`. `main` đúng với
    /// `-m`: gói được chạy qua `__main__.py`. `dir` là `--app-dir`/`--chdir`.
    Module { name: String, main: bool, dir: Option<String> },
    /// Đường dẫn file như lệnh viết, tương đối thì tính từ thư mục làm việc:
    /// script Python hay JavaScript, hoặc script shell cần đọc tiếp.
    Script(String),
    /// `npm start`, `yarn dev`: tên script trong `package.json`.
    Npm(String),
    /// `go run ./cmd/x`.
    GoRun(String),
    /// Trình chạy có điểm vào theo quy ước: file đầu tiên có thật trong danh
    /// sách, tính từ thư mục làm việc. `react-scripts start` là `src/index.*`.
    OneOf(Vec<String>),
    /// `flask run` không có `--app`: app lấy từ biến `FLASK_APP`, không có thì
    /// `wsgi.py` rồi `app.py`.
    FlaskApp,
    /// Chương trình không nhận ra, như binary Go dựng sẵn.
    Program(String),
    /// Có lệnh chạy ở đây mà không đọc tiếp được (lồng quá sâu). Bên gọi báo
    /// ra, không coi như không có gì: chỗ dừng đọc là chỗ giấu mã được.
    Opaque(String),
}

/// Trình chạy Python và bọc ngoài: theo sau chúng là một lệnh khác.
const WRAPPERS: &[&str] = &[
    "exec", "env", "nohup", "time", "nice", "ionice", "stdbuf", "tini", "dumb-init", "catatonit",
    "opentelemetry-instrument", "ddtrace-run", "newrelic-admin", "run-program", "watchmedo",
    "auto-restart", "sudo", "doppler", "infisical", "op", "aws-vault",
];
/// Bọc ngoài nhận đúng một đối số trước lệnh thật.
const WRAPPERS_WITH_ARG: &[&str] = &["gosu", "su-exec", "chpst", "timeout", "setpriv"];
/// `uv run`, `poetry run`…: từ thứ hai là `run`, rồi tới lệnh thật.
const RUNNERS: &[&str] = &["uv", "poetry", "pipenv", "pdm", "hatch", "rye", "conda", "micromamba", "mamba", "pixi"];
const SHELLS: &[&str] = &["sh", "bash", "dash", "ash", "zsh", "ksh"];
const ASGI: &[&str] = &["uvicorn", "gunicorn", "hypercorn", "daphne", "granian", "waitress-serve"];
/// Lệnh mà file script đứng sau, kể cả sau vài cờ: `torchrun --nproc 8 train.py`.
const SCRIPT_RUNNERS: &[&str] = &[
    "streamlit", "chainlit", "panel", "gradio", "solara", "mesop", "torchrun", "accelerate",
    "deepspeed", "mpirun", "horovodrun", "ray", "nodemon", "node", "nodejs", "bun", "deno",
    "ts-node", "tsx", "pm2-runtime", "pm2", "python", "fastapi",
];

/// Giới hạn lồng `sh -c "sh -c ..."`: chuỗi người khác viết, không tin được.
const MAX_NESTING: usize = 4;

fn too_deep() -> Launch {
    Launch::Opaque(format!("lệnh lồng quá {MAX_NESTING} tầng"))
}

fn base(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// `"$PYTHON_CMD"`, `${PYTHON}`: script khởi chạy hay chọn trình thông dịch
/// vào một biến rồi gọi qua biến đó.
fn is_python_var(word: &str) -> bool {
    word.strip_prefix('$')
        .map(|v| v.trim_start_matches('{').trim_end_matches('}'))
        .is_some_and(|v| v.to_ascii_uppercase().contains("PYTHON"))
}

fn is_python(word: &str) -> bool {
    let b = base(word);
    (b.starts_with("python") || b.starts_with("pypy"))
        && b.trim_start_matches("python").trim_start_matches("pypy").chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// `a.b.c` hay `a.b:app`, phần module là định danh Python nối bằng dấu chấm.
fn module_of(target: &str) -> Option<&str> {
    let module = target.split(':').next()?;
    let ok = !module.is_empty()
        && module.split('.').all(|seg| {
            !seg.is_empty()
                && seg.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                && seg.chars().all(|c| c.is_alphanumeric() || c == '_')
        });
    ok.then_some(module)
}

fn is_file_like(word: &str) -> bool {
    const EXTS: &[&str] = &[".py", ".js", ".mjs", ".cjs", ".ts", ".mts", ".sh", ".bash"];
    EXTS.iter().any(|e| word.ends_with(e))
}

/// Những gì một dãy đối số (`ENTRYPOINT` nối `CMD`) nạp.
pub fn launches(argv: &[String]) -> Vec<Launch> {
    let mut out = Vec::new();
    words(argv, 0, false, &mut out);
    unique(out)
}

/// Bỏ trùng, giữ thứ tự. Bảng băm chứ không so từng cặp: một script 1 MB có
/// thể có hàng trăm nghìn lệnh.
fn unique(items: Vec<Launch>) -> Vec<Launch> {
    let mut seen = std::collections::HashSet::new();
    items.into_iter().filter(|l| seen.insert(l.clone())).collect()
}

/// Những gì một đoạn văn bản shell nạp: chuỗi của `sh -c`, hay nội dung một
/// file script khởi chạy.
pub fn launches_in_shell(text: &str) -> Vec<Launch> {
    let mut out = Vec::new();
    shell(text, 0, &mut out);
    unique(out)
}

/// `cd`/`pushd` đổi thư mục cho các lệnh sau trong cùng đoạn shell: `cd
/// mlapp/train && python -m loop` chạy `mlapp/train/loop.py`. Thư mục là biến
/// không biết thì đường dẫn sau đó không tìm ra được — và bên gọi báo là không
/// rõ điểm vào, thay vì đoán.
fn shell(text: &str, depth: usize, out: &mut Vec<Launch>) {
    if depth > MAX_NESTING {
        out.push(too_deep());
        return;
    }
    let mut cwd: Option<String> = None;
    for segment in segments(text) {
        let ws = shell_words(&segment);
        let start = ws.iter().position(|w| !KEYWORDS.contains(&w.as_str())).unwrap_or(ws.len());
        if matches!(ws.get(start).map(String::as_str), Some("cd" | "pushd")) {
            let dir = ws.get(start + 1).filter(|d| !d.starts_with('-')).cloned().unwrap_or_else(|| "~".into());
            cwd = Some(match &cwd {
                Some(c) if !dir.starts_with('/') && !dir.starts_with('~') => format!("{c}/{dir}"),
                _ => dir,
            });
            continue;
        }
        let mut found = Vec::new();
        words(&ws, depth + 1, true, &mut found);
        if let Some(dir) = &cwd {
            found.iter_mut().for_each(|l| relocate(l, dir));
        }
        out.extend(found);
    }
}

/// Lệnh chạy sau `cd dir`: đường dẫn tương đối và module tính từ `dir`.
fn relocate(launch: &mut Launch, dir: &str) {
    let join = |p: &str| if p.starts_with('/') { p.to_string() } else { format!("{dir}/{p}") };
    match launch {
        Launch::Module { dir: d, .. } => *d = Some(d.as_deref().map_or_else(|| dir.to_string(), join)),
        Launch::Script(p) | Launch::GoRun(p) => *p = join(p),
        Launch::Program(p) if p.starts_with("./") => *p = join(p),
        Launch::OneOf(c) => c.iter_mut().for_each(|p| *p = join(p)),
        _ => {}
    }
}

/// Tách văn bản shell thành từng lệnh đơn ở `&&`, `||`, `;`, `|`, `&`, xuống
/// dòng, ngoài dấu nháy. Bỏ chú thích.
pub(crate) fn segments(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    let mut quote: Option<char> = None;
    let mut at_word_start = true;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            cur.push(c);
            if c == '\\' && q == '"' {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                cur.push(c);
            }
            '\\' => {
                // Nối dòng trong shell.
                match chars.next() {
                    Some('\n') => cur.push(' '),
                    Some(n) => {
                        cur.push('\\');
                        cur.push(n);
                    }
                    None => {}
                }
            }
            '#' if at_word_start => {
                while chars.peek().is_some_and(|&n| n != '\n') {
                    chars.next();
                }
            }
            '\n' | ';' | '|' | '&' => {
                if !cur.trim().is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
            }
            c => cur.push(c),
        }
        at_word_start = c.is_whitespace() || matches!(c, ';' | '|' | '&' | '\n');
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Từ khoá của shell đứng trước một lệnh.
const KEYWORDS: &[&str] = &["if", "then", "else", "elif", "do", "while", "until", "!", "{", "(", "exec", "command", "builtin"];

/// `in_shell`: dãy này là một lệnh trong văn bản shell. Ở đó phần lớn lệnh là
/// tiện ích (`set`, `mkdir`, `[[`), nên chương trình lạ chỉ được tính khi viết
/// bằng đường dẫn.
fn words(argv: &[String], depth: usize, in_shell: bool, out: &mut Vec<Launch>) {
    if depth > MAX_NESTING {
        if !argv.is_empty() {
            out.push(too_deep());
        }
        return;
    }
    let mut i = 0;
    // Bỏ từ khoá shell, gán biến `A=b`, chuyển hướng và bọc ngoài.
    loop {
        let Some(w) = argv.get(i) else { return };
        let b = base(w);
        if KEYWORDS.contains(&w.as_str()) || is_assignment(w) || is_redirect(w) || w == "--" {
            i += 1;
        } else if WRAPPERS.contains(&b) {
            i += 1;
            // Cờ của bọc ngoài: `env -i`, `tini -g --`, `timeout -s KILL 10`.
            while argv.get(i).is_some_and(|w| w.starts_with('-') || is_assignment(w)) {
                i += 1;
            }
        } else if WRAPPERS_WITH_ARG.contains(&b) {
            i += 1;
            while argv.get(i).is_some_and(|w| w.starts_with('-')) {
                i += 1;
            }
            i += 1;
        } else if RUNNERS.contains(&b) && argv.get(i + 1).is_some_and(|w| w == "run") {
            i += 2;
            // `uv run --with x python …`, `conda run -n env python …`: bỏ cờ và
            // giá trị của cờ cho tới khi gặp một trình chạy nhận ra được.
            while let Some(w) = argv.get(i) {
                if !w.starts_with('-') {
                    break;
                }
                i += 1;
                if !w.contains('=') && argv.get(i).is_some_and(|v| !v.starts_with('-') && !known(v)) {
                    i += 1;
                }
            }
        } else {
            break;
        }
    }
    let rest = &argv[i..];
    let Some(head) = rest.first() else { return };
    let b = base(head);

    if SHELLS.contains(&b) {
        // `sh -c "<lệnh>"`, `bash -ec "<lệnh>"`, `bash -o pipefail -c "<lệnh>"`;
        // hay `bash script.sh`. Như bash: `-c` nghĩa là đối số không phải tuỳ
        // chọn đầu tiên là chuỗi lệnh; `-o`, `-O` lấy thêm tên tuỳ chọn.
        let mut j = 1;
        let mut command = false;
        while let Some(w) = rest.get(j) {
            if w.starts_with("--") {
                j += 1;
            } else if (w.starts_with('-') || w.starts_with('+')) && w.len() > 1 {
                command |= w.contains('c');
                j += if w.contains(['o', 'O']) { 2 } else { 1 };
            } else if command {
                shell(w, depth + 1, out);
                return;
            } else {
                out.push(Launch::Script(w.clone()));
                // Đối số sau script thường là lệnh mà nó `exec "$@"`.
                words(after_script(&rest[j + 1..]), depth + 1, in_shell, out);
                return;
            }
        }
        return;
    }
    if is_python(head) || is_python_var(head) {
        python(&rest[1..], depth, in_shell, out);
        return;
    }
    if ASGI.contains(&b) {
        asgi(&rest[1..], out);
        return;
    }
    match b {
        "celery" => {
            if let Some(app) = flag_value(&rest[1..], &["-A", "--app"])
                && let Some(m) = module_of(&app)
            {
                out.push(Launch::Module { name: m.to_string(), main: false, dir: flag_value(&rest[1..], &["--workdir"]) });
            }
            return;
        }
        "npm" | "yarn" | "pnpm" | "bun" if rest.len() > 1 => {
            let args: Vec<&String> = rest[1..].iter().filter(|w| !w.starts_with('-')).collect();
            match args.as_slice() {
                [first, ..] if *first == "start" || *first == "test" => out.push(Launch::Npm(first.to_string())),
                [run, name, ..] if *run == "run" || *run == "run-script" => out.push(Launch::Npm(name.to_string())),
                [name, ..] if b != "npm" && !is_file_like(name) => out.push(Launch::Npm(name.to_string())),
                _ => {}
            }
            if b != "bun" || !rest[1..].iter().any(|w| is_file_like(w)) {
                return;
            }
        }
        "go" if rest.get(1).is_some_and(|w| w == "run") => {
            if let Some(pkg) = rest[2..].iter().find(|w| !w.starts_with('-')) {
                out.push(Launch::GoRun(pkg.clone()));
            }
            return;
        }
        "flask" => {
            match flag_value(&rest[1..], &["--app", "-A"]) {
                Some(app) => push_target(&app, out),
                None => out.push(Launch::FlaskApp),
            }
            return;
        }
        "react-scripts" | "craco" if rest.get(1).is_some_and(|w| w == "start" || w == "build") => {
            out.push(Launch::OneOf(conventional("src/index")));
            return;
        }
        "vite" => {
            out.push(Launch::OneOf(conventional("src/main")));
            return;
        }
        "vue-cli-service" if rest.get(1).is_some_and(|w| w == "serve" || w == "build") => {
            out.push(Launch::OneOf(conventional("src/main")));
            return;
        }
        "cargo" if rest.get(1).is_some_and(|w| w == "run") => {
            out.push(Launch::OneOf(vec!["src/main.rs".into()]));
            return;
        }
        "ng" if rest.get(1).is_some_and(|w| w == "serve" || w == "build") => {
            out.push(Launch::OneOf(vec!["src/main.ts".into()]));
            return;
        }
        "fastapi" => {
            if let Some(target) = rest[1..].iter().filter(|w| !w.starts_with('-')).nth(1) {
                push_target(target, out);
            }
            return;
        }
        _ => {}
    }
    if SCRIPT_RUNNERS.contains(&b) {
        // `torchrun -m pkg.train` hay `torchrun --nproc 8 train.py`.
        if let Some(m) = flag_value(&rest[1..], &["-m", "--module"]).as_deref().and_then(module_of) {
            out.push(Launch::Module { name: m.to_string(), main: true, dir: None });
        } else if let Some(s) = rest[1..].iter().find(|w| is_file_like(w)) {
            out.push(Launch::Script(s.clone()));
        }
        return;
    }
    if is_file_like(head) || head.starts_with("./") {
        out.push(Launch::Script(head.clone()));
        words(after_script(&rest[1..]), depth + 1, in_shell, out);
        return;
    }
    // Không nhận ra: có thể là binary Go dựng sẵn, bên gọi tự đối chiếu.
    if !head.starts_with('-') && !head.contains('$') && (!in_shell || head.contains('/')) {
        out.push(Launch::Program(head.clone()));
    }
}

/// Lệnh mà một script chạy tiếp qua `exec "$@"`. `wait-for-it.sh db:5432 --
/// python app.py`: đối số đầu là `host:cổng` hay một cờ thì lệnh nằm sau
/// `--`; không có `--` thì bỏ các `host:cổng` đứng đầu.
fn after_script(args: &[String]) -> &[String] {
    let host_port = |w: &String| {
        w.rsplit_once(':').is_some_and(|(h, p)| {
            !h.is_empty() && !h.contains('/') && !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())
        })
    };
    match args.first() {
        Some(w) if host_port(w) || w.starts_with('-') => match args.iter().position(|w| w == "--") {
            Some(i) => &args[i + 1..],
            None => &args[args.iter().take_while(|w| host_port(w)).count()..],
        },
        _ => args,
    }
}

/// Từ này là một trình chạy mà `words` biết cách đọc.
fn known(word: &str) -> bool {
    let b = base(word);
    is_python(word) || ASGI.contains(&b) || SCRIPT_RUNNERS.contains(&b) || b == "celery"
}

fn is_assignment(w: &str) -> bool {
    match w.split_once('=') {
        Some((name, _)) => {
            !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.starts_with(char::is_numeric)
        }
        None => false,
    }
}

fn is_redirect(w: &str) -> bool {
    let t = w.trim_start_matches(|c: char| c.is_ascii_digit());
    t.starts_with('>') || t.starts_with('<')
}

/// Module mà `python -m` chạy như một trình chạy: `python -m uvicorn app:app`
/// là `uvicorn app:app`.
const RUNNER_MODULES: &[(&str, &str)] = &[
    ("uvicorn", "uvicorn"),
    ("gunicorn", "gunicorn"),
    ("hypercorn", "hypercorn"),
    ("celery", "celery"),
    ("flask", "flask"),
    ("streamlit", "streamlit"),
    ("fastapi", "fastapi"),
    ("gradio", "gradio"),
    ("torch.distributed.run", "torchrun"),
    ("torch.distributed.launch", "torchrun"),
    ("accelerate.commands.launch", "accelerate"),
];

/// `python [cờ] -m a.b` hay `python [cờ] script.py`.
fn python(args: &[String], depth: usize, in_shell: bool, out: &mut Vec<Launch>) {
    let mut i = 0;
    while let Some(w) = args.get(i) {
        match w.as_str() {
            "-m" => {
                let Some(m) = args.get(i + 1).and_then(|m| module_of(m)) else { return };
                match RUNNER_MODULES.iter().find(|(module, _)| *module == m) {
                    Some((_, runner)) => {
                        let mut argv = vec![runner.to_string()];
                        if *runner == "accelerate" {
                            argv.push("launch".into());
                        }
                        argv.extend(args[i + 2..].iter().cloned());
                        words(&argv, depth + 1, in_shell, out);
                    }
                    None => out.push(Launch::Module { name: m.to_string(), main: true, dir: None }),
                }
                return;
            }
            "-c" => return, // mã viết thẳng trong lệnh
            "-W" | "-X" | "--check-hash-based-pycs" => i += 2,
            w if w.starts_with("-m") && w.len() > 2 => {
                if let Some(m) = module_of(&w[2..]) {
                    out.push(Launch::Module { name: m.to_string(), main: true, dir: None });
                }
                return;
            }
            w if w.starts_with('-') => i += 1,
            w => {
                out.push(Launch::Script(w.to_string()));
                return;
            }
        }
    }
}

/// `uvicorn [cờ] a.b:app`, `gunicorn -k x a.b:app`.
fn asgi(args: &[String], out: &mut Vec<Launch>) {
    let dir = flag_value(args, &["--app-dir", "--chdir"]);
    let target = args
        .iter()
        .find(|w| !w.starts_with('-') && w.contains(':') && module_of(w).is_some())
        .or_else(|| {
            // Không có `:`: `gunicorn app` hay `gunicorn app.wsgi`. Lấy từ cuối
            // cùng trông như module và không phải giá trị của một cờ.
            args.iter()
                .enumerate()
                .filter(|(n, w)| {
                    !w.starts_with('-')
                        && module_of(w).is_some()
                        && (*n == 0 || !args[n - 1].starts_with('-') || args[n - 1].contains('='))
                })
                .map(|(_, w)| w)
                .next_back()
        });
    if let Some(t) = target.and_then(|t| module_of(t)) {
        out.push(Launch::Module { name: t.to_string(), main: false, dir });
    }
}

fn conventional(stem: &str) -> Vec<String> {
    ["tsx", "ts", "jsx", "js"].iter().map(|e| format!("{stem}.{e}")).collect()
}

/// `a.b:app` là module, `app/main.py` là file.
fn push_target(target: &str, out: &mut Vec<Launch>) {
    if is_file_like(target.split(':').next().unwrap_or(target)) {
        out.push(Launch::Script(target.split(':').next().unwrap_or(target).to_string()));
    } else if let Some(m) = module_of(target) {
        out.push(Launch::Module { name: m.to_string(), main: false, dir: None });
    }
}

/// Giá trị của cờ: `-A x`, `--app x`, `--app=x`, `-Ax`.
fn flag_value(args: &[String], names: &[&str]) -> Option<String> {
    for (n, w) in args.iter().enumerate() {
        for name in names {
            if w == name {
                return args.get(n + 1).cloned();
            }
            if let Some(v) = w.strip_prefix(&format!("{name}=")) {
                return Some(v.to_string());
            }
            if name.len() == 2
                && !name.starts_with("--")
                && let Some(v) = w.strip_prefix(name)
                && !v.is_empty()
                && !w.starts_with("--")
            {
                return Some(v.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        shell_words(s)
    }

    fn module(name: &str) -> Launch {
        Launch::Module { name: name.into(), main: false, dir: None }
    }

    fn main_module(name: &str) -> Launch {
        Launch::Module { name: name.into(), main: true, dir: None }
    }

    #[test]
    fn python_va_trinh_chay_asgi() {
        assert_eq!(launches(&argv("python -u -m app.worker --x")), [main_module("app.worker")]);
        assert_eq!(launches(&argv("/usr/local/bin/python3.12 -W ignore manage.py runserver")), [Launch::Script("manage.py".into())]);
        assert_eq!(launches(&argv("python -c 'import x'")), []);
        assert_eq!(launches(&argv("uvicorn --host 0.0.0.0 --port 80 app.main:app --reload")), [module("app.main")]);
        assert_eq!(
            launches(&argv("gunicorn -k uvicorn.workers.UvicornWorker -b :8000 --chdir src app:create_app()")),
            [Launch::Module { name: "app".into(), main: false, dir: Some("src".into()) }]
        );
        assert_eq!(launches(&argv("gunicorn --workers 4 myproj.wsgi")), [module("myproj.wsgi")]);
        assert_eq!(launches(&argv("celery -A app.celery worker -l info")), [module("app.celery")]);
        assert_eq!(launches(&argv("celery --app=tasks beat")), [module("tasks")]);
    }

    #[test]
    fn boc_ngoai_va_trinh_quan_ly_moi_truong() {
        assert_eq!(launches(&argv("exec env A=1 B=2 python app.py")), [Launch::Script("app.py".into())]);
        assert_eq!(launches(&argv("uv run --with x --frozen uvicorn api:app")), [module("api")]);
        assert_eq!(launches(&argv("poetry run python -m svc")), [main_module("svc")]);
        assert_eq!(launches(&argv("tini -- gosu app opentelemetry-instrument python main.py")), [Launch::Script("main.py".into())]);
        assert_eq!(launches(&argv("timeout 30 node server.js")), [Launch::Script("server.js".into())]);
    }

    #[test]
    fn shell_va_script_khoi_chay() {
        assert_eq!(
            launches(&argv("/bin/sh -c 'python manage.py migrate && gunicorn app.wsgi:application'")),
            [Launch::Script("manage.py".into()), module("app.wsgi")]
        );
        assert_eq!(
            launches(&["/bin/bash".into(), "/entrypoint.sh".into(), "python".into(), "-m".into(), "app".into()]),
            [Launch::Script("/entrypoint.sh".into()), main_module("app")]
        );
        let script = "#!/bin/bash\nset -e\nif [[ \"${MODE}\" == \"worker\" ]]; then\n  exec celery -A app.celery worker \\\n    -c 2\nelse\n  exec gunicorn --bind 0.0.0.0:5001 app:app\nfi\n";
        assert_eq!(launches_in_shell(script), [module("app.celery"), module("app")]);
        assert_eq!(
            launches_in_shell("PYTHON_CMD=$(command -v python3)\nexec env A=\"${A:-}\" \"$PYTHON_CMD\" -m uvicorn open_webui.main:app"),
            [module("open_webui.main")],
            "`python -m uvicorn X` là `uvicorn X`"
        );
        assert_eq!(launches(&argv("python -m torch.distributed.run --nproc_per_node 2 train.py")), [Launch::Script("train.py".into())]);
        assert_eq!(
            launches(&argv("sh -c 'cd mlapp/train && PYTHONPATH=/app python -m loop'")),
            [Launch::Module { name: "loop".into(), main: true, dir: Some("mlapp/train".into()) }],
            "`cd` đổi thư mục cho lệnh sau"
        );
        assert_eq!(launches_in_shell("cd /srv\ncd api\n./run.sh"), [Launch::Script("/srv/api/./run.sh".into())]);
    }

    #[test]
    fn script_ml_va_node() {
        assert_eq!(launches(&argv("torchrun --nproc_per_node 8 train.py --cfg x")), [Launch::Script("train.py".into())]);
        assert_eq!(launches(&argv("accelerate launch -m pkg.train")), [main_module("pkg.train")]);
        assert_eq!(launches(&argv("streamlit run ui/app.py")), [Launch::Script("ui/app.py".into())]);
        assert_eq!(launches(&argv("fastapi run app/main.py")), [Launch::Script("app/main.py".into())]);
        assert_eq!(launches(&argv("nodemon --inspect=0.0.0.0 server.js")), [Launch::Script("server.js".into())]);
        assert_eq!(launches(&argv("npm run start:prod")), [Launch::Npm("start:prod".into())]);
        assert_eq!(launches(&argv("yarn dev")), [Launch::Npm("dev".into())]);
        assert_eq!(launches(&argv("go run ./cmd/api")), [Launch::GoRun("./cmd/api".into())]);
        assert_eq!(launches(&argv("flask run --host 0.0.0.0")), [Launch::FlaskApp]);
        assert_eq!(launches(&argv("flask --app web.app:create_app run")), [module("web.app")]);
        assert_eq!(launches_in_shell("react-scripts start"), [Launch::OneOf(conventional("src/index"))]);
        assert_eq!(launches(&argv("/go/bin/frontend")), [Launch::Program("/go/bin/frontend".into())]);
    }

    #[test]
    fn script_chay_tiep_lenh_sau_no() {
        let pay = Launch::Module { name: "mlapp.pay".into(), main: true, dir: None };
        let wait = Launch::Script("./wait-for-it.sh".into());
        assert_eq!(launches(&argv("./wait-for-it.sh db:5432 -- python -m mlapp.pay")), [wait.clone(), pay.clone()]);
        assert_eq!(launches(&argv("./wait-for-it.sh db:5432 -t 30 -- python -m mlapp.pay")), [wait.clone(), pay.clone()]);
        assert_eq!(launches(&argv("./wait-for-it.sh db:5432 python -m mlapp.pay")), [wait, pay.clone()]);
        assert_eq!(
            launches(&argv("bash entrypoint.sh python -m mlapp.pay")),
            [Launch::Script("entrypoint.sh".into()), pay],
            "không có host:cổng thì đối số là lệnh"
        );
    }

    #[test]
    fn tuy_chon_cua_shell() {
        assert_eq!(launches(&argv("bash -o pipefail -c 'python -m mlapp.api'")), api_main(), "-o lấy tên tuỳ chọn");
        assert_eq!(launches(&argv("bash -eo pipefail -c 'python -m mlapp.api'")), api_main());
        assert_eq!(launches(&argv("bash -c -e 'python -m mlapp.api'")), api_main(), "-c rồi tuỳ chọn khác");
        assert_eq!(launches(&argv("sh -O extglob run.sh")), [Launch::Script("run.sh".into())]);
    }

    fn api_main() -> Vec<Launch> {
        vec![Launch::Module { name: "mlapp.api".into(), main: true, dir: None }]
    }

    #[test]
    fn long_sh_c_co_gioi_han() {
        // Mỗi tầng nháy đơn nhân số dấu nháy lên ba, nên mười tầng là đủ.
        let mut cmd = "python app.py".to_string();
        for _ in 0..10 {
            cmd = format!("sh -c {}", shell_quote(&cmd));
        }
        // Thôi đọc, nhưng nói ra là đã thôi (TM-12): lệnh trong cùng có thể
        // chạy mã ngoài `runs`.
        assert_eq!(launches(&argv(&cmd)), [too_deep()], "lồng quá {MAX_NESTING} cấp thì thôi đọc và báo");
        let mut cmd = "python app.py".to_string();
        for _ in 0..MAX_NESTING / 2 {
            cmd = format!("sh -c {}", shell_quote(&cmd));
        }
        assert_eq!(launches(&argv(&cmd)), [Launch::Script("app.py".into())], "trong giới hạn thì đọc tới cùng");
    }

    fn shell_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
