//! Slosh's KDL subset. Values remain strings until configuration interprets them.
use std::{fmt, path::Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub args: Vec<String>,
    /// Kept in source order: duplicate keys are allowed and the last wins.
    pub props: Vec<(String, String)>,
    pub kids: Vec<Node>,
    pub line: usize,
}

/// Authoritative layout values retain bytes; text consumers opt into a view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeNode {
    pub name: Vec<u8>,
    pub args: Vec<Vec<u8>>,
    pub props: Vec<(Vec<u8>, Vec<u8>)>,
    pub kids: Vec<NativeNode>,
    pub line: usize,
}

impl NativeNode {
    fn new(name: Vec<u8>, line: usize) -> Self {
        Self {
            name,
            args: Vec::new(),
            props: Vec::new(),
            kids: Vec::new(),
            line,
        }
    }
    pub(crate) fn child(&self, name: &str) -> Option<&Self> {
        self.kids.iter().find(|n| n.name == name.as_bytes())
    }
    pub(crate) fn property(&self, key: &str) -> Option<&[u8]> {
        self.props
            .iter()
            .rev()
            .find(|(k, _)| k == key.as_bytes())
            .map(|(_, v)| v.as_slice())
    }
    pub(crate) fn prop<'a>(&'a self, key: &str, fallback: &'a [u8]) -> &'a [u8] {
        self.property(key).unwrap_or(fallback)
    }
    pub(crate) fn prop_int(&self, key: &str, fallback: i64) -> i64 {
        self.property(key)
            .and_then(|v| integer(&String::from_utf8_lossy(v)))
            .unwrap_or(fallback)
    }
    pub(crate) fn prop_bool(&self, key: &str, fallback: bool) -> bool {
        self.property(key)
            .and_then(|v| std::str::from_utf8(v).ok())
            .and_then(boolean)
            .unwrap_or(fallback)
    }
    pub(crate) fn text(self) -> Node {
        let display = |v: Vec<u8>| String::from_utf8_lossy(&v).into_owned();
        let mut node = Node::new(display(self.name), self.line);
        node.args = self.args.into_iter().map(display).collect();
        node.props = self
            .props
            .into_iter()
            .map(|(k, v)| (display(k), display(v)))
            .collect();
        node.kids = self.kids.into_iter().map(Self::text).collect();
        node
    }
}

impl Node {
    fn new(name: String, line: usize) -> Self {
        Self {
            name,
            args: Vec::new(),
            props: Vec::new(),
            kids: Vec::new(),
            line,
        }
    }

    pub fn child(&self, name: &str) -> Option<&Self> {
        self.kids.iter().find(|child| child.name == name)
    }
    pub fn arg<'a>(&'a self, index: usize, fallback: &'a str) -> &'a str {
        self.args.get(index).map(String::as_str).unwrap_or(fallback)
    }
    pub fn arg_int(&self, index: usize, fallback: i64) -> i64 {
        self.args
            .get(index)
            .and_then(|value| integer(value))
            .unwrap_or(fallback)
    }
    pub fn arg_bool(&self, index: usize, fallback: bool) -> bool {
        self.args
            .get(index)
            .and_then(|value| boolean(value))
            .unwrap_or(fallback)
    }
    pub fn property(&self, key: &str) -> Option<&str> {
        self.props
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
    pub fn prop<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        self.property(key).unwrap_or(fallback)
    }
    pub fn prop_int(&self, key: &str, fallback: i64) -> i64 {
        self.property(key).and_then(integer).unwrap_or(fallback)
    }
    pub fn prop_bool(&self, key: &str, fallback: bool) -> bool {
        self.property(key).and_then(boolean).unwrap_or(fallback)
    }
}

/// Match decimal strtol, including numeric prefixes and saturation on overflow.
pub(crate) fn integer(text: &str) -> Option<i64> {
    let text = text.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '\u{b}');
    let bytes = text.as_bytes();
    let sign_len = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let count = bytes[sign_len..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count();
    if count == 0 {
        return None;
    }
    Some(text[..sign_len + count].parse().unwrap_or_else(|_| {
        if text.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        }
    }))
}

pub(crate) fn boolean(text: &str) -> Option<bool> {
    match text {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub message: &'static str,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}
impl std::error::Error for Error {}

struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
    line: usize,
}

fn space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r')
}
fn identifier(byte: u8) -> bool {
    !space(byte) && !matches!(byte, 0 | b'\n' | b'{' | b'}' | b'=' | b';' | b'"')
}

impl Parser<'_> {
    fn peek(&self) -> u8 {
        self.bytes.get(self.offset).copied().unwrap_or(0)
    }
    fn next(&self) -> u8 {
        self.bytes.get(self.offset + 1).copied().unwrap_or(0)
    }
    fn error(&self, message: &'static str) -> Error {
        Error {
            line: self.line,
            message,
        }
    }

    fn skip(&mut self, newlines: bool) {
        loop {
            match self.peek() {
                byte if space(byte) => self.offset += 1,
                b'\n' if newlines => {
                    self.offset += 1;
                    self.line += 1;
                }
                b'/' if self.next() == b'/' => {
                    while !matches!(self.peek(), 0 | b'\n') {
                        self.offset += 1;
                    }
                }
                b'/' if self.next() == b'*' => {
                    self.offset += 2;
                    while self.peek() != 0 && !(self.peek() == b'*' && self.next() == b'/') {
                        if self.peek() == b'\n' {
                            self.line += 1;
                        }
                        self.offset += 1;
                    }
                    if self.peek() != 0 {
                        self.offset += 2;
                    }
                }
                b'\\' if self.next() == b'\n' => {
                    self.offset += 2;
                    self.line += 1;
                }
                _ => return,
            }
        }
    }

    fn bare(&mut self, message: &'static str) -> Result<Vec<u8>, Error> {
        let start = self.offset;
        while identifier(self.peek()) {
            self.offset += 1;
        }
        if self.offset == start {
            return Err(self.error(message));
        }
        Ok(self.bytes[start..self.offset].to_vec())
    }

    fn value(&mut self) -> Result<Vec<u8>, Error> {
        if self.peek() != b'"' {
            return self.bare("expected a value");
        }
        self.offset += 1;
        let mut output = Vec::new();
        while !matches!(self.peek(), 0 | b'"') {
            let mut byte = self.peek();
            self.offset += 1;
            if byte == b'\\' {
                let escape = self.peek();
                if escape == 0 {
                    break;
                }
                self.offset += 1;
                byte = match escape {
                    b'n' => b'\n',
                    b't' => b'\t',
                    b'r' => b'\r',
                    b'e' => 0x1b,
                    other => other,
                };
            }
            if byte == b'\n' {
                self.line += 1;
            }
            output.push(byte);
        }
        if self.peek() != b'"' {
            return Err(self.error("unterminated string"));
        }
        self.offset += 1;
        Ok(output)
    }

    fn node(&mut self, depth: usize) -> Result<NativeNode, Error> {
        let line = self.line;
        let mut node = NativeNode::new(self.bare("expected a node name")?, line);
        loop {
            self.skip(false);
            match self.peek() {
                0 | b'\n' | b';' | b'}' => return Ok(node),
                b'{' => {
                    self.offset += 1;
                    node.kids = self.nodes(true, depth + 1)?;
                    return Ok(node);
                }
                _ => {
                    let first = self.value()?;
                    if self.peek() == b'=' {
                        self.offset += 1;
                        node.props.push((first, self.value()?));
                    } else {
                        node.args.push(first);
                    }
                }
            }
        }
    }

    fn nodes(&mut self, block: bool, depth: usize) -> Result<Vec<NativeNode>, Error> {
        // Configuration is untrusted; bound recursion rather than overflowing the stack.
        if depth > 256 {
            return Err(self.error("nesting too deep"));
        }
        let mut nodes = Vec::new();
        loop {
            self.skip(true);
            match self.peek() {
                0 if block => return Err(self.error("unclosed {")),
                0 => return Ok(nodes),
                b'}' => {
                    self.offset += 1;
                    return if block {
                        Ok(nodes)
                    } else {
                        Err(self.error("unexpected }"))
                    };
                }
                b';' => self.offset += 1,
                _ => nodes.push(self.node(depth)?),
            }
        }
    }
}

pub fn parse(text: &str) -> Result<Node, Error> {
    parse_bytes(text)
}

pub(crate) fn parse_bytes(text: impl AsRef<[u8]>) -> Result<Node, Error> {
    parse_native(text).map(NativeNode::text)
}

pub(crate) fn parse_native(text: impl AsRef<[u8]>) -> Result<NativeNode, Error> {
    let mut parser = Parser {
        bytes: text.as_ref(),
        offset: 0,
        line: 1,
    };
    let mut root = NativeNode::new(Vec::new(), 1);
    root.kids = parser.nodes(false, 0)?;
    Ok(root)
}

pub fn parse_file(path: impl AsRef<Path>) -> Result<Node, Box<dyn std::error::Error>> {
    Ok(parse_native(std::fs::read(path)?)?.text())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_kdl_suite_document() {
        let doc = "// a comment\ngap 1\nrounded true\ntitle \"hello world\"\nmin_pane cols=24 rows=6\n/* block\n   comment */\ntheme {\n  frame_focus \"#ff5fd7\"\n  nested { deep \"value\" }\n}\noneline { a \"1\"; b \"2\" }\n";
        let root = parse(doc).unwrap();
        assert_eq!(root.child("gap").unwrap().arg_int(0, -1), 1);
        assert!(root.child("rounded").unwrap().arg_bool(0, false));
        assert_eq!(root.child("title").unwrap().arg(0, ""), "hello world");
        let size = root.child("min_pane").unwrap();
        assert_eq!(
            (size.prop_int("cols", 0), size.prop_int("rows", 0)),
            (24, 6)
        );
        let theme = root.child("theme").unwrap();
        assert_eq!(theme.kids.len(), 2);
        assert_eq!(theme.child("frame_focus").unwrap().arg(0, ""), "#ff5fd7");
        assert_eq!(
            theme
                .child("nested")
                .unwrap()
                .child("deep")
                .unwrap()
                .arg(0, ""),
            "value"
        );
        assert_eq!(root.child("oneline").unwrap().kids.len(), 2);
    }

    #[test]
    fn original_kdl_suite_errors_and_escapes() {
        assert_eq!(
            parse("a \"x\\ty\\nz\\\"q\"\n")
                .unwrap()
                .child("a")
                .unwrap()
                .arg(0, ""),
            "x\ty\nz\"q"
        );
        assert_eq!(
            parse("keys {\n bind \"ctrl+a\" \"quit\"\n")
                .unwrap_err()
                .to_string(),
            "line 3: unclosed {"
        );
        assert_eq!(
            parse("a \"unterminated\n").unwrap_err().to_string(),
            "line 2: unterminated string"
        );
        assert!(parse("").unwrap().kids.is_empty());
    }

    #[test]
    fn continuation_duplicate_properties_and_numeric_prefixes() {
        let root = parse("size cols=1 cols=\"24px\" \\\n rows=-6\n").unwrap();
        let node = &root.kids[0];
        assert_eq!(node.prop_int("cols", 0), 24);
        assert_eq!(node.prop_int("rows", 0), -6);
        assert_eq!(integer("9223372036854775808"), Some(i64::MAX));
        assert_eq!(integer("-9223372036854775809"), Some(i64::MIN));
        assert_eq!(integer("no"), None);
    }

    #[test]
    fn trailing_backslash_and_excessive_nesting_are_errors() {
        assert!(parse("a \"\\").is_err());
        assert!(parse(&format!("{}{}", "a {".repeat(300), "}".repeat(300))).is_err());
        assert!(parse("}").is_err());
    }
}
