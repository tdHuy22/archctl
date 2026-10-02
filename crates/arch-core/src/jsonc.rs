//! Đọc JSON có chú thích, đủ dùng cho `tsconfig.json`.
//!
//! `tsconfig.json` không phải JSON chuẩn: nó cho phép `//`, `/* */` và dấu
//! phẩy thừa. Viết tay khoảng trăm dòng rẻ hơn kéo thêm một dependency vào
//! công cụ chạy trong repo của người dùng.
//!
//! Không nhằm thay thế một bộ đọc JSON đầy đủ: số được giữ nguyên dạng chuỗi
//! vì ở đây không chỗ nào cần giá trị số.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Số và chuỗi gộp làm một: nơi dùng chỉ cần đọc chuỗi.
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Lấy theo đường dẫn lồng nhau: `get_path(["compilerOptions", "paths"])`.
    pub fn get_path<'a>(&'a self, keys: &[&str]) -> Option<&'a Value> {
        keys.iter().try_fold(self, |node, key| node.get(key))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Obj(fields) => Some(fields),
            _ => None,
        }
    }
}

/// `None` khi nguồn không đọc được. Lỗi cụ thể không quan trọng ở đây: nơi gọi
/// chỉ cần biết có dùng được cấu hình này hay không.
pub fn parse(src: &str) -> Option<Value> {
    let chars: Vec<char> = src.chars().collect();
    let mut p = Parser { cs: &chars, i: 0 };
    p.skip_trivia();
    let value = p.value()?;
    p.skip_trivia();
    (p.i >= p.cs.len()).then_some(value)
}

struct Parser<'a> {
    cs: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.cs.get(self.i).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            return true;
        }
        false
    }

    /// Khoảng trắng và chú thích. `tsconfig.json` dùng cả hai kiểu chú thích.
    fn skip_trivia(&mut self) {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.i += 1;
            }
            match (self.peek(), self.cs.get(self.i + 1)) {
                (Some('/'), Some('/')) => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.i += 1;
                    }
                }
                (Some('/'), Some('*')) => {
                    self.i += 2;
                    while self.i < self.cs.len()
                        && !(self.cs[self.i] == '*' && self.cs.get(self.i + 1) == Some(&'/'))
                    {
                        self.i += 1;
                    }
                    self.i = (self.i + 2).min(self.cs.len());
                }
                _ => return,
            }
        }
    }

    fn value(&mut self) -> Option<Value> {
        match self.peek()? {
            '{' => self.object(),
            '[' => self.array(),
            '"' => self.string().map(Value::Str),
            _ => self.literal(),
        }
    }

    fn object(&mut self) -> Option<Value> {
        self.i += 1;
        let mut fields = Vec::new();
        loop {
            self.skip_trivia();
            if self.eat('}') {
                return Some(Value::Obj(fields));
            }
            let key = self.string()?;
            self.skip_trivia();
            if !self.eat(':') {
                return None;
            }
            self.skip_trivia();
            fields.push((key, self.value()?));
            self.skip_trivia();
            // Dấu phẩy thừa trước `}` là hợp lệ trong tsconfig.
            if !self.eat(',') && self.peek() != Some('}') {
                return None;
            }
        }
    }

    fn array(&mut self) -> Option<Value> {
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            if self.eat(']') {
                return Some(Value::Arr(items));
            }
            items.push(self.value()?);
            self.skip_trivia();
            if !self.eat(',') && self.peek() != Some(']') {
                return None;
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        if !self.eat('"') {
            return None;
        }
        let mut out = String::new();
        loop {
            match self.peek()? {
                '"' => {
                    self.i += 1;
                    return Some(out);
                }
                '\\' => {
                    self.i += 1;
                    let escaped = self.peek()?;
                    self.i += 1;
                    out.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'u' => self.unicode()?,
                        c => c,
                    });
                }
                c => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn unicode(&mut self) -> Option<char> {
        let hex: String = self.cs.get(self.i..self.i + 4)?.iter().collect();
        self.i += 4;
        char::from_u32(u32::from_str_radix(&hex, 16).ok()?)
    }

    /// `true`, `false`, `null`, hoặc một số. Số giữ nguyên dạng chuỗi.
    fn literal(&mut self) -> Option<Value> {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_alphanumeric() || "+-.eE".contains(c))
        {
            self.i += 1;
        }
        let text: String = self.cs[start..self.i].iter().collect();
        match text.as_str() {
            "" => None,
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            "null" => Some(Value::Null),
            other => Some(Value::Str(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_duoc_tsconfig_that() {
        let src = r#"{
  // chú thích một dòng
  "extends": "./tsconfig.base.json",
  /* nhiều
     dòng */
  "compilerOptions": {
    "baseUrl": ".",
    "strict": true,
    "target": "ES2022",
    "paths": {
      "@/*": ["./src/*"],
      "~/lib": ["src/lib/index.ts", "src/lib/mod.ts"],
    },
  },
}"#;
        let v = parse(src).expect("đọc được");
        assert_eq!(v.get("extends").and_then(Value::as_str), Some("./tsconfig.base.json"));
        assert_eq!(
            v.get_path(&["compilerOptions", "baseUrl"]).and_then(Value::as_str),
            Some(".")
        );
        let paths = v.get_path(&["compilerOptions", "paths"]).unwrap();
        assert_eq!(paths.as_object().unwrap().len(), 2);
        assert_eq!(
            paths.get("~/lib").and_then(Value::as_array).map(<[Value]>::len),
            Some(2)
        );
    }

    #[test]
    fn chuoi_co_escape() {
        let v = parse(r#"{"a": "x\ny", "b": "A"}"#).unwrap();
        assert_eq!(v.get("a").and_then(Value::as_str), Some("x\ny"));
        assert_eq!(v.get("b").and_then(Value::as_str), Some("A"));
    }

    #[test]
    fn nguon_hong_tra_ve_none() {
        assert!(parse("{").is_none());
        assert!(parse(r#"{"a" 1}"#).is_none());
        assert!(parse("").is_none());
        assert!(parse(r#"{"a": 1} rác"#).is_none());
    }

    #[test]
    fn mang_long_nhau_va_rong() {
        let v = parse(r#"{"a": [], "b": [[1, 2], []]}"#).unwrap();
        assert_eq!(v.get("a").and_then(Value::as_array).map(<[Value]>::len), Some(0));
        assert_eq!(v.get("b").and_then(Value::as_array).map(<[Value]>::len), Some(2));
    }
}
