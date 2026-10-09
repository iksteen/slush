//! Control requests retain JSON numbers until each command applies C's limits.
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::BTreeMap,
    ffi::{OsStr, OsString},
};

/// Native strings are protocol bytes on Unix, just as in the C JSON writer.
pub(crate) enum Reply {
    Value(Value),
    Native(OsString),
    Bytes(Vec<u8>),
    Array(Vec<Reply>),
    Object(BTreeMap<String, Reply>),
}

impl From<Value> for Reply {
    fn from(value: Value) -> Self {
        match value {
            Value::Object(fields) => {
                Self::Object(fields.into_iter().map(|(k, v)| (k, v.into())).collect())
            }
            Value::Array(items) => Self::Array(items.into_iter().map(Self::from).collect()),
            value => Self::Value(value),
        }
    }
}

pub(crate) fn native_bytes(value: &OsStr) -> Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Cow::Borrowed(value.as_bytes())
    }
    #[cfg(windows)]
    {
        Cow::Owned(value.to_string_lossy().into_owned().into_bytes())
    }
}

impl Reply {
    pub(crate) fn native(value: impl AsRef<OsStr>) -> Self {
        Self::Native(value.as_ref().to_owned())
    }
    pub(crate) fn string_bytes(value: impl AsRef<[u8]>) -> Self {
        Self::Bytes(value.as_ref().to_vec())
    }
    pub(crate) fn field(mut self, key: &str, value: Self) -> Self {
        let Self::Object(fields) = &mut self else {
            unreachable!("reply fields require an object")
        };
        fields.insert(key.into(), value);
        self
    }
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }
    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Self::Value(value) => {
                serde_json::to_writer(out, value).expect("a byte vector accepts JSON output")
            }
            Self::Native(value) => write_string(out, &native_bytes(value)),
            Self::Bytes(value) => write_string(out, value),
            Self::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    item.write(out);
                }
                out.push(b']');
            }
            Self::Object(fields) => {
                out.push(b'{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index != 0 {
                        out.push(b',');
                    }
                    serde_json::to_writer(&mut *out, key)
                        .expect("a byte vector accepts JSON output");
                    out.push(b':');
                    value.write(out);
                }
                out.push(b'}');
            }
        }
    }
}

fn write_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'"');
    for byte in bytes.iter().copied() {
        match byte {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0..=0x1f => {
                const HEX: &[u8] = b"0123456789abcdef";
                out.extend_from_slice(b"\\u00");
                out.extend([HEX[(byte >> 4) as usize], HEX[(byte & 15) as usize]]);
            }
            byte => out.push(byte),
        }
    }
    out.push(b'"');
}

/// Control strings retain their bytes, including escapes and embedded NULs.
#[derive(Debug)]
pub(crate) enum Request {
    Object(Vec<(Vec<u8>, Request)>),
    Array(Vec<Request>),
    String(Vec<u8>),
    Scalar(Value),
}

pub(crate) fn os_string(bytes: &[u8]) -> Cow<'_, OsStr> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Cow::Borrowed(OsStr::from_bytes(bytes))
    }
    #[cfg(windows)]
    {
        Cow::Owned(OsString::from(String::from_utf8_lossy(bytes).as_ref()))
    }
}

fn c_string(bytes: &[u8]) -> &[u8] {
    bytes.split(|b| *b == 0).next().unwrap_or_default()
}

impl Request {
    pub(crate) fn get(&self, key: &str) -> Option<&Self> {
        let Self::Object(fields) = self else {
            return None;
        };
        fields
            .iter()
            .rev()
            .find(|(k, _)| c_string(k) == key.as_bytes())
            .map(|(_, v)| v)
    }
    pub(crate) fn as_bytes(&self) -> Option<&[u8]> {
        if let Self::String(bytes) = self {
            Some(c_string(bytes))
        } else {
            None
        }
    }
    pub(crate) fn as_os(&self) -> Option<Cow<'_, OsStr>> {
        self.as_bytes().map(os_string)
    }
    pub(crate) fn value(&self) -> Value {
        match self {
            Self::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(k, v)| (String::from_utf8_lossy(c_string(k)).into_owned(), v.value()))
                    .collect(),
            ),
            Self::Array(items) => Value::Array(items.iter().map(Self::value).collect()),
            Self::String(bytes) => {
                Value::String(String::from_utf8_lossy(c_string(bytes)).into_owned())
            }
            Self::Scalar(value) => value.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ParseError;

pub(crate) fn parse_request(text: &[u8]) -> Result<Request, ParseError> {
    // strtod needs a terminator; the original command protocol also uses C strings.
    let mut input = c_string(text).to_vec();
    input.push(0);
    let mut parser = RequestParser { input, offset: 0 };
    let request = parser.value(0)?;
    parser.space();
    if parser.input[parser.offset] != 0 {
        return Err(ParseError);
    }
    Ok(request)
}

struct RequestParser {
    input: Vec<u8>,
    offset: usize,
}
impl RequestParser {
    fn space(&mut self) {
        while matches!(self.input[self.offset], b' ' | b'\t' | b'\r' | b'\n') {
            self.offset += 1;
        }
    }
    fn take(&mut self, byte: u8) -> bool {
        self.space();
        if self.input[self.offset] == byte {
            self.offset += 1;
            true
        } else {
            false
        }
    }
    fn value(&mut self, depth: usize) -> Result<Request, ParseError> {
        if depth >= 128 {
            return Err(ParseError);
        }
        self.space();
        match self.input[self.offset] {
            b'{' => {
                self.offset += 1;
                let mut fields = Vec::new();
                if self.take(b'}') {
                    return Ok(Request::Object(fields));
                }
                loop {
                    self.space();
                    let key = self.string()?;
                    if !self.take(b':') {
                        return Err(ParseError);
                    }
                    fields.push((key, self.value(depth + 1)?));
                    if self.take(b'}') {
                        return Ok(Request::Object(fields));
                    }
                    if !self.take(b',') {
                        return Err(ParseError);
                    }
                }
            }
            b'[' => {
                self.offset += 1;
                let mut items = Vec::new();
                if self.take(b']') {
                    return Ok(Request::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if self.take(b']') {
                        return Ok(Request::Array(items));
                    }
                    if !self.take(b',') {
                        return Err(ParseError);
                    }
                }
            }
            b'"' => self.string().map(Request::String),
            _ => {
                for (word, value) in [
                    (b"true".as_slice(), Value::Bool(true)),
                    (b"false", Value::Bool(false)),
                    (b"null", Value::Null),
                ] {
                    if self.input[self.offset..].starts_with(word) {
                        self.offset += word.len();
                        return Ok(Request::Scalar(value));
                    }
                }
                let start = self.offset;
                let mut end = std::ptr::null_mut();
                // SAFETY: input has a trailing NUL, offset is within it, and end
                // is writable. strtod only reads through the terminating NUL.
                let number =
                    unsafe { libc::strtod(self.input.as_ptr().add(start).cast(), &mut end) };
                // SAFETY: strtod returns a pointer within this same input buffer.
                let length = unsafe { end.offset_from(self.input.as_ptr().add(start).cast()) };
                if length <= 0 {
                    return Err(ParseError);
                }
                self.offset += length as usize;
                let raw =
                    std::str::from_utf8(&self.input[start..self.offset]).map_err(|_| ParseError)?;
                let value = if let Ok(number) = raw.parse::<serde_json::Number>() {
                    Value::Number(number)
                } else if number.is_infinite() {
                    Value::Number(
                        if number.is_sign_negative() {
                            "-1e999"
                        } else {
                            "1e999"
                        }
                        .parse()
                        .map_err(|_| ParseError)?,
                    )
                } else {
                    Value::from(number)
                };
                Ok(Request::Scalar(value))
            }
        }
    }
    fn hex4(&mut self) -> Result<u32, ParseError> {
        let digits = self
            .input
            .get(self.offset..self.offset + 4)
            .ok_or(ParseError)?;
        if digits.contains(&0) {
            return Err(ParseError);
        }
        let value = digits.iter().fold(0u32, |value, byte| {
            (value << 4)
                | match byte {
                    b'0'..=b'9' => u32::from(byte - b'0'),
                    b'a'..=b'f' => u32::from(byte - b'a' + 10),
                    b'A'..=b'F' => u32::from(byte - b'A' + 10),
                    _ => 0,
                }
        });
        self.offset += 4;
        Ok(value)
    }
    fn string(&mut self) -> Result<Vec<u8>, ParseError> {
        if !self.take(b'"') {
            return Err(ParseError);
        }
        let mut bytes = Vec::new();
        loop {
            let byte = self.input[self.offset];
            self.offset += 1;
            match byte {
                0 => return Err(ParseError),
                b'"' => return Ok(bytes),
                b'\\' => {
                    let escaped = self.input[self.offset];
                    self.offset += 1;
                    match escaped {
                        b'n' => bytes.push(b'\n'),
                        b'r' => bytes.push(b'\r'),
                        b't' => bytes.push(b'\t'),
                        b'b' => bytes.push(8),
                        b'f' => bytes.push(12),
                        b'/' | b'"' | b'\\' => bytes.push(escaped),
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xd800..0xdc00).contains(&code)
                                && self.input.get(self.offset..self.offset + 2) == Some(b"\\u")
                                && self
                                    .input
                                    .get(self.offset + 2..self.offset + 6)
                                    .is_some_and(|p| !p.contains(&0))
                            {
                                self.offset += 2;
                                let low = self.hex4()?;
                                code = 0x10000u32
                                    .wrapping_add((code - 0xd800) << 10)
                                    .wrapping_add(low.wrapping_sub(0xdc00));
                            }
                            if code < 0x80 {
                                bytes.push(code as u8);
                            } else if code < 0x800 {
                                bytes.extend([
                                    (0xc0 | (code >> 6)) as u8,
                                    (0x80 | (code & 63)) as u8,
                                ]);
                            } else if code < 0x10000 {
                                bytes.extend([
                                    (0xe0 | (code >> 12)) as u8,
                                    (0x80 | ((code >> 6) & 63)) as u8,
                                    (0x80 | (code & 63)) as u8,
                                ]);
                            } else {
                                bytes.extend([
                                    (0xf0 | (code >> 18)) as u8,
                                    (0x80 | ((code >> 12) & 63)) as u8,
                                    (0x80 | ((code >> 6) & 63)) as u8,
                                    (0x80 | (code & 63)) as u8,
                                ]);
                            }
                        }
                        _ => return Err(ParseError),
                    }
                }
                byte => bytes.push(byte),
            }
        }
    }
}

#[cfg(test)]
fn parse(text: &str) -> Result<Value, ParseError> {
    parse_request(text.as_bytes()).map(|request| request.value())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_request_strings_keep_bytes_and_c_string_boundaries() {
        let request = parse_request(b"{\"path\":\"/tmp/\xff\\\"\\\\\\t\",\"data\":\"A\\u0000ignored\",\"cmd\\u0000ignored\":\"alive\"}").unwrap();
        assert_eq!(
            request.get("path").unwrap().as_bytes(),
            Some(b"/tmp/\xff\"\\\t".as_slice())
        );
        assert_eq!(
            request.get("data").unwrap().as_bytes(),
            Some(b"A".as_slice())
        );
        assert_eq!(
            request.get("cmd").unwrap().as_bytes(),
            Some(b"alive".as_slice())
        );
    }

    #[test]
    fn request_escape_decoding_matches_the_reference_byte_parser() {
        let request =
            parse_request(br#"{"valid":"\u00e9\ud83d\ude80","surrogate":"\udcff","hex":"\u00zg"}"#)
                .unwrap();
        assert_eq!(
            request.get("valid").unwrap().as_bytes(),
            Some("é🚀".as_bytes())
        );
        assert_eq!(
            request.get("surrogate").unwrap().as_bytes(),
            Some(b"\xed\xb3\xbf".as_slice())
        );
        assert_eq!(request.get("hex").unwrap().as_bytes(), Some(b"".as_slice()));
    }

    #[test]
    fn truncated_requests_fail_without_reading_past_the_buffer() {
        for text in [
            b"{\"x\":\"\\".as_slice(),
            br#"{"x":"\u1"}"#,
            br#"{"x":"\ud800\u1"}"#,
            b"{",
            b"{\"x\":[1,}",
            b"{\"x\":1} garbage",
        ] {
            assert!(parse_request(text).is_err(), "{text:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_reply_fields_escape_controls_without_replacing_filename_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let entry = Reply::from(serde_json::json!({"path":"ordinary placeholder"})).field(
            "path",
            Reply::native(OsStr::from_bytes(b"/tmp/\xff\"\\\t\r\n\x00\x01\x08\x0c")),
        );
        let reply = Reply::from(
            serde_json::json!({"ordinary":{"$serde_json::private::Number":"user value"}}),
        )
        .field("entries", Reply::Array(vec![entry]));
        assert_eq!(reply.bytes(), b"{\"entries\":[{\"path\":\"/tmp/\xff\\\"\\\\\\t\\r\\n\\u0000\\u0001\\u0008\\u000c\"}],\"ordinary\":{\"$serde_json::private::Number\":\"user value\"}}");
    }

    #[test]
    fn large_numbers_survive_nested_control_requests() {
        let request = parse(r#" {"cmd":"alive","ignored":[1e999,-1e999]} "#).unwrap();
        assert_eq!(request["ignored"][0].to_string(), "1e+999");
        assert_eq!(request["ignored"][1].to_string(), "-1e+999");
    }

    #[test]
    fn number_metadata_keys_remain_ordinary_objects() {
        let request = parse(r#"{"cols":{"$serde_json::private::Number":"40"},"cols":{}}"#).unwrap();
        assert_eq!(request["cols"], serde_json::json!({}));
        let request = parse(r#"{"cols":{"$serde_json::private::Number":"40"}}"#).unwrap();
        assert!(request["cols"].is_object());
        assert_eq!(request["cols"]["$serde_json::private::Number"], "40");
    }
}
