//! Bounded, incremental side-channel scanner. The same bytes also go to VT.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Verb(Vec<u8>, Vec<u8>),
    Status(Report),
    Query { bel: bool },
    Prompt,
    Reset,
}

#[derive(Default)]
pub struct Scanner {
    state: u8,
    body: Vec<u8>,
    overflow: bool,
}
impl Scanner {
    pub fn feed(&mut self, data: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        for &c in data {
            match (self.state, c) {
                (0, 0x1b) => self.state = 1,
                (1 | 3, b']') => {
                    self.state = 2;
                    self.body.clear();
                    self.overflow = false;
                }
                (1 | 3, b'c') => {
                    events.push(Event::Reset);
                    self.state = 0;
                }
                (1, 0x1b) => {}
                (2, 0x07) | (3, b'\\') => {
                    if !self.overflow
                        && let Some(event) = dispatch(&self.body, c == 0x07)
                    {
                        events.push(event);
                    }
                    self.state = 0;
                }
                (1 | 3, _) => self.state = 0,
                (2, 0x1b) => self.state = 3,
                (2, 0..=0x1f) => self.state = 0,
                (2, _) => {
                    if self.body.len() < 4096 {
                        self.body.push(c);
                    } else {
                        self.overflow = true;
                    }
                }
                _ => {}
            }
        }
        events
    }
}
fn dispatch(body: &[u8], bel: bool) -> Option<Event> {
    if let Some(rest) = body.strip_prefix(b"5577;1;") {
        let mut parts = rest.splitn(2, |c| *c == b';');
        let verb = parts.next()?;
        if verb.len() >= 32 || verb.ends_with(b"-reply") {
            return None;
        }
        return Some(Event::Verb(
            verb.into(),
            parts.next().unwrap_or_default().into(),
        ));
    }
    if let Some(rest) = body.strip_prefix(b"7501;") {
        if rest == b"?" {
            return Some(Event::Query { bel });
        }
        return Report::parse(rest).map(Event::Status);
    }
    if body == b"133;A" || body.starts_with(b"133;A;") {
        return Some(Event::Prompt);
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
    Clear,
}
impl State {
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Error => "error",
            Self::Clear => "",
        }
    }
    pub fn busy(self) -> bool {
        matches!(self, Self::Working | Self::Blocked)
    }
    fn rank(self) -> u8 {
        match self {
            Self::Clear => 0,
            Self::Idle => 1,
            Self::Working => 2,
            Self::Done => 3,
            Self::Error => 4,
            Self::Blocked => 5,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub state: State,
    pub kind: String,
    pub progress: i32,
    pub id: String,
    pub app: String,
    pub title: String,
    pub msg: String,
}
fn name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"_.+-".contains(&c)
}
fn trim_pair(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(|c| matches!(c, b' ' | b'\t')) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(|c| matches!(c, b' ' | b'\t')) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}
impl Report {
    pub fn parse(pairs: &[u8]) -> Option<Self> {
        if pairs.len() > 4092 {
            return None;
        }
        let mut r = Self {
            state: State::Idle,
            kind: String::new(),
            progress: -1,
            id: String::new(),
            app: String::new(),
            title: String::new(),
            msg: String::new(),
        };
        let mut state = false;
        for pair in pairs.split(|c| *c == b':') {
            let pair = trim_pair(pair);
            let Some(eq) = pair.iter().position(|c| *c == b'=') else {
                continue;
            };
            let k = trim_pair(&pair[..eq]);
            let v = trim_pair(&pair[eq + 1..]);
            if k.is_empty() {
                continue;
            }
            if k.len() > 16 {
                return None;
            }
            if !k.iter().all(|c| c.is_ascii_lowercase())
                || !v.iter().all(|c| name_byte(*c) || b",/=".contains(c))
            {
                continue;
            }
            // Both slices are now known to contain only ASCII grammar bytes.
            let k = std::str::from_utf8(k).ok()?;
            let v = std::str::from_utf8(v).ok()?;
            match k {
                "state" => {
                    r.state = match v {
                        "idle" => State::Idle,
                        "working" => State::Working,
                        "done" => State::Done,
                        "blocked" => State::Blocked,
                        "error" => State::Error,
                        "clear" => State::Clear,
                        _ => return None,
                    };
                    state = true;
                }
                "id" => {
                    let segments: Vec<_> = v.split('/').collect();
                    if v.len() > 128
                        || (!v.is_empty()
                            && (segments.len() > 8
                                || segments.iter().any(|s| {
                                    s.is_empty() || s.len() > 32 || !s.bytes().all(name_byte)
                                })))
                    {
                        return None;
                    }
                    r.id = v.into();
                }
                "app" => {
                    if v.len() > 32 {
                        return None;
                    }
                    r.app = if v.bytes().all(name_byte) {
                        v.into()
                    } else {
                        String::new()
                    };
                }
                "kind" => {
                    r.kind = if matches!(v, "permission" | "question" | "auth") {
                        v.into()
                    } else {
                        String::new()
                    }
                }
                "progress" => {
                    r.progress =
                        if !v.is_empty() && v.len() <= 3 && v.bytes().all(|c| c.is_ascii_digit()) {
                            v.parse::<i32>().ok().filter(|n| *n <= 100).unwrap_or(-1)
                        } else {
                            -1
                        };
                }
                "msg" | "title" => {
                    let msg = k == "msg";
                    if v.len() > if msg { 2732 } else { 256 } {
                        return None;
                    }
                    let dec = decode_base64(v, if msg { 2048 } else { 192 })?;
                    let s = std::str::from_utf8(&dec).ok()?;
                    if s.chars().any(char::is_control) {
                        return None;
                    }
                    let kept = shorten(s, 255);
                    if msg {
                        r.msg = kept;
                    } else {
                        r.title = kept;
                    }
                }
                _ => {}
            }
        }
        if !state {
            return None;
        }
        if r.state != State::Blocked {
            r.kind.clear();
        }
        if !r.state.busy() {
            r.progress = -1;
        }
        Some(r)
    }
    pub fn line(&self) -> String {
        String::from_utf8_lossy(&self.line_bytes()).into_owned()
    }
    pub fn line_bytes(&self) -> Vec<u8> {
        let text = match (self.title.is_empty(), self.msg.is_empty()) {
            (false, false) => format!("{}: {}", self.title, self.msg),
            (false, true) => self.title.clone(),
            (true, false) => self.msg.clone(),
            (true, true) => self.state.name().into(),
        };
        let mut line = text.as_bytes()[..text.len().min(255)].to_vec();
        if self.progress >= 0 && line.len() + 6 < 256 {
            line.extend_from_slice(format!(" {}%", self.progress).as_bytes());
        }
        line
    }
}
pub fn shorten(text: &str, cap: usize) -> String {
    let mut end = cap.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].into()
}
pub fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        out.push(ALPHABET[a >> 2] as char);
        out.push(ALPHABET[(a & 3) << 4 | b >> 4] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(b & 15) << 2 | c >> 6] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[c & 63] as char
        } else {
            '='
        });
    }
    out
}

fn decode_base64(text: &str, cap: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    let mut padded = false;
    for c in text.bytes() {
        if c == b'=' {
            padded = true;
            continue;
        }
        if padded {
            return None;
        }
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            if out.len() == cap {
                return None;
            }
            out.push((acc >> bits) as u8);
        }
    }
    if bits >= 6 || (bits > 0 && acc & ((1 << bits) - 1) != 0) {
        return None;
    }
    Some(out)
}
pub fn descendant(id: &str, parent: &str) -> bool {
    if parent.is_empty() {
        !id.is_empty()
    } else {
        id.strip_prefix(parent)
            .is_some_and(|rest| rest.starts_with('/'))
    }
}
#[derive(Default)]
pub struct Records {
    entries: Vec<(Report, u64)>,
    clock: u64,
    pub showing: Option<Report>,
}
impl Records {
    pub fn apply(&mut self, report: Report) {
        if report.state == State::Clear {
            self.entries.retain(|(r, _)| {
                !report.id.is_empty() && r.id != report.id && !descendant(&r.id, &report.id)
            });
        } else {
            self.clock += 1;
            let at = self
                .entries
                .iter()
                .position(|(r, _)| r.id == report.id)
                .or_else(|| {
                    (self.entries.len() == 64).then(|| {
                        self.entries
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, (_, seen))| *seen)
                            .unwrap()
                            .0
                    })
                });
            if let Some(at) = at {
                self.entries[at] = (report, self.clock);
            } else {
                self.entries.push((report, self.clock));
            }
        }
        self.recompose();
    }
    pub fn drop_transient(&mut self) {
        self.entries.retain(|(r, _)| !r.state.busy());
        self.recompose();
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.showing = None;
    }
    fn recompose(&mut self) {
        self.showing = self
            .entries
            .iter()
            .max_by_key(|(r, seen)| (r.state.rank(), *seen))
            .map(|(r, _)| r.clone());
        if let Some(show) = &mut self.showing
            && show.app.is_empty()
            && let Some((ancestor, _)) = self
                .entries
                .iter()
                .filter(|(r, _)| !r.app.is_empty() && descendant(&show.id, &r.id))
                .max_by_key(|(r, _)| r.id.len())
        {
            show.app.clone_from(&ancestor.app);
        }
    }
}

#[derive(Clone)]
pub struct Button {
    pub id: String,
    pub label: Vec<u8>,
}
pub fn buttons(payload: impl AsRef<[u8]>) -> Vec<Button> {
    payload
        .as_ref()
        .split(|b| *b == b';')
        .filter_map(|field| {
            let colon = field.iter().position(|b| *b == b':')?;
            let id = unescape_bytes(&field[..colon], 255);
            let label = unescape_bytes(&field[colon + 1..], 32);
            if id.is_empty()
                || id.len() > 32
                || !id
                    .iter()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(c))
                || label.is_empty()
            {
                return None;
            }
            Some(Button {
                id: String::from_utf8(id).expect("validated ASCII button id"),
                label,
            })
        })
        .take(8)
        .collect()
}
pub fn unescape(text: &str, cap: usize) -> String {
    String::from_utf8_lossy(&unescape_bytes(text.as_bytes(), cap)).into_owned()
}
pub fn unescape_bytes(bytes: &[u8], cap: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() && out.len() < cap {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let end = out.iter().position(|c| *c == 0).unwrap_or(out.len());
    out.truncate(end);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_and_hostile_sequences() {
        let mut scanner = Scanner::default();
        assert!(scanner.feed(b"\x1b]7501;state=work").is_empty());
        let events = scanner.feed(b"ing:msg=aGVsbG8=\x1b\\");
        assert!(matches!(&events[..], [Event::Status(r)] if r.msg == "hello"));
        let oversized = format!("\x1b]5577;1;status;{}\x07", "x".repeat(4096));
        assert!(scanner.feed(oversized.as_bytes()).is_empty());
        assert_eq!(
            scanner.feed(b"\x1b]7501;?\x07"),
            [Event::Query { bel: true }]
        );
        assert!(
            scanner
                .feed(b"\x1b]5577;1;hello-reply;slosh;1\x07")
                .is_empty()
        );
        assert!(Report::parse(b"state=working:msg=YcKbYg==").is_none());
        assert!(Report::parse(b"state=working:id=a//b").is_none());
    }
    #[test]
    fn record_hierarchy_and_lifetime() {
        let mut records = Records::default();
        for text in [
            "state=working:app=cargo",
            "state=blocked:id=build/test:msg=cXVlc3Rpb24=",
        ] {
            records.apply(Report::parse(text.as_bytes()).unwrap());
        }
        let show = records.showing.as_ref().unwrap();
        assert_eq!(
            (show.state, show.app.as_str(), show.msg.as_str()),
            (State::Blocked, "cargo", "question")
        );
        records.apply(Report::parse(b"state=clear:id=build").unwrap());
        assert_eq!(records.showing.as_ref().unwrap().state, State::Working);
        records.drop_transient();
        assert!(records.showing.is_none());
    }
}
