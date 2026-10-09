//! Incremental decoding of outer-terminal input into Ghostty semantic events.
use crate::ghostty::ffi::*;

pub const SHIFT: u16 = 1;
pub const CTRL: u16 = 2;
pub const ALT: u16 = 4;
pub const SUPER: u16 = 8;
pub const CAPS: u16 = 16;
pub const NUM: u16 = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key {
        key: GhosttyKey,
        mods: u16,
        action: u8,
        text: Vec<u8>,
        unshifted: u32,
    },
    Mouse {
        button: u8,
        action: u8,
        x: u16,
        y: u16,
        mods: u16,
    },
    Paste(Vec<u8>),
    Focus(bool),
}

fn key_event(key: GhosttyKey, mods: u16, action: u8, text: &[u8], unshifted: u32) -> Event {
    Event::Key {
        key,
        mods,
        action,
        text: text[..text.len().min(16)].to_vec(),
        unshifted,
    }
}

const SHIFT_PAIRS: &[(u8, u8)] = &[
    (b'~', b'`'),
    (b'_', b'-'),
    (b'+', b'='),
    (b'{', b'['),
    (b'}', b']'),
    (b'|', b'\\'),
    (b':', b';'),
    (b'"', b'\''),
    (b'<', b','),
    (b'>', b'.'),
    (b'?', b'/'),
    (b'!', b'1'),
    (b'@', b'2'),
    (b'#', b'3'),
    (b'$', b'4'),
    (b'%', b'5'),
    (b'^', b'6'),
    (b'&', b'7'),
    (b'*', b'8'),
    (b'(', b'9'),
    (b')', b'0'),
];

fn ascii_key(c: u8) -> GhosttyKey {
    let c = SHIFT_PAIRS
        .iter()
        .find(|(shifted, _)| *shifted == c)
        .map_or(c, |(_, base)| *base);
    match c {
        b'a'..=b'z' => GHOSTTY_KEY_A + (c - b'a') as i32,
        b'A'..=b'Z' => GHOSTTY_KEY_A + (c - b'A') as i32,
        b'0'..=b'9' => GHOSTTY_KEY_DIGIT_0 + (c - b'0') as i32,
        b' ' => GHOSTTY_KEY_SPACE,
        b'`' => GHOSTTY_KEY_BACKQUOTE,
        b'-' => GHOSTTY_KEY_MINUS,
        b'=' => GHOSTTY_KEY_EQUAL,
        b'[' => GHOSTTY_KEY_BRACKET_LEFT,
        b']' => GHOSTTY_KEY_BRACKET_RIGHT,
        b'\\' => GHOSTTY_KEY_BACKSLASH,
        b';' => GHOSTTY_KEY_SEMICOLON,
        b'\'' => GHOSTTY_KEY_QUOTE,
        b',' => GHOSTTY_KEY_COMMA,
        b'.' => GHOSTTY_KEY_PERIOD,
        b'/' => GHOSTTY_KEY_SLASH,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}

fn tilde_key(number: i32) -> GhosttyKey {
    match number {
        1 | 7 => GHOSTTY_KEY_HOME,
        2 => GHOSTTY_KEY_INSERT,
        3 => GHOSTTY_KEY_DELETE,
        4 | 8 => GHOSTTY_KEY_END,
        5 => GHOSTTY_KEY_PAGE_UP,
        6 => GHOSTTY_KEY_PAGE_DOWN,
        11 => GHOSTTY_KEY_F1,
        12 => GHOSTTY_KEY_F2,
        13 => GHOSTTY_KEY_F3,
        14 => GHOSTTY_KEY_F4,
        15 => GHOSTTY_KEY_F5,
        17 => GHOSTTY_KEY_F6,
        18 => GHOSTTY_KEY_F7,
        19 => GHOSTTY_KEY_F8,
        20 => GHOSTTY_KEY_F9,
        21 => GHOSTTY_KEY_F10,
        23 => GHOSTTY_KEY_F11,
        24 => GHOSTTY_KEY_F12,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}
fn final_key(final_byte: u8) -> GhosttyKey {
    match final_byte {
        b'A' => GHOSTTY_KEY_ARROW_UP,
        b'B' => GHOSTTY_KEY_ARROW_DOWN,
        b'C' => GHOSTTY_KEY_ARROW_RIGHT,
        b'D' => GHOSTTY_KEY_ARROW_LEFT,
        b'E' => GHOSTTY_KEY_NUMPAD_BEGIN,
        b'H' => GHOSTTY_KEY_HOME,
        b'F' => GHOSTTY_KEY_END,
        b'P' => GHOSTTY_KEY_F1,
        b'Q' => GHOSTTY_KEY_F2,
        b'R' => GHOSTTY_KEY_F3,
        b'S' => GHOSTTY_KEY_F4,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}
fn kitty_key(cp: u32) -> GhosttyKey {
    match cp {
        27 => GHOSTTY_KEY_ESCAPE,
        13 => GHOSTTY_KEY_ENTER,
        9 => GHOSTTY_KEY_TAB,
        127 => GHOSTTY_KEY_BACKSPACE,
        57358 => GHOSTTY_KEY_CAPS_LOCK,
        57359 => GHOSTTY_KEY_SCROLL_LOCK,
        57360 => GHOSTTY_KEY_NUM_LOCK,
        57361 => GHOSTTY_KEY_PRINT_SCREEN,
        57362 => GHOSTTY_KEY_PAUSE,
        57363 => GHOSTTY_KEY_CONTEXT_MENU,
        57441 => GHOSTTY_KEY_SHIFT_LEFT,
        57442 => GHOSTTY_KEY_CONTROL_LEFT,
        57443 => GHOSTTY_KEY_ALT_LEFT,
        57444 => GHOSTTY_KEY_META_LEFT,
        57447 => GHOSTTY_KEY_SHIFT_RIGHT,
        57448 => GHOSTTY_KEY_CONTROL_RIGHT,
        57449 => GHOSTTY_KEY_ALT_RIGHT,
        57450 => GHOSTTY_KEY_META_RIGHT,
        57376..=57398 => GHOSTTY_KEY_F13 + (cp - 57376) as i32,
        0..=127 => ascii_key(cp as u8),
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    }
}
fn decode_mods(value: i32) -> u16 {
    let mut result = 0;
    for (bit, modifier) in [
        (1, SHIFT),
        (2, ALT),
        (4, CTRL),
        (8, SUPER),
        (64, CAPS),
        (128, NUM),
    ] {
        if value & bit != 0 {
            result |= modifier;
        }
    }
    result
}

/// Grow integer parameters without allowing malformed input to overflow.
fn digit(value: &mut i32, byte: u8) {
    if byte.is_ascii_digit() && *value <= i32::MAX / 10 {
        *value = value
            .saturating_mul(10)
            .saturating_add((byte - b'0') as i32);
    }
}

fn encode_cp(cp: u32) -> Vec<u8> {
    // Preserve slosh's byte encoding even for codepoints outside Unicode's range.
    if cp < 0x80 {
        vec![cp as u8]
    } else if cp < 0x800 {
        vec![(0xc0 | (cp >> 6)) as u8, (0x80 | (cp & 63)) as u8]
    } else if cp < 0x10000 {
        vec![
            (0xe0 | (cp >> 12)) as u8,
            (0x80 | ((cp >> 6) & 63)) as u8,
            (0x80 | (cp & 63)) as u8,
        ]
    } else {
        vec![
            (0xf0 | (cp >> 18)) as u8,
            (0x80 | ((cp >> 12) & 63)) as u8,
            (0x80 | ((cp >> 6) & 63)) as u8,
            (0x80 | (cp & 63)) as u8,
        ]
    }
}

fn csi(params: &[u8], final_byte: u8) -> Option<Event> {
    if matches!(final_byte, b'I' | b'O') {
        return Some(Event::Focus(final_byte == b'I'));
    }
    if params.first() == Some(&b'<') && matches!(final_byte, b'M' | b'm') {
        let mut fields = [0; 3];
        let mut index = 0;
        for &byte in &params[1..] {
            if index >= 3 {
                break;
            }
            if byte == b';' {
                index += 1;
            } else {
                digit(&mut fields[index], byte);
            }
        }
        let button = fields[0];
        let mut mods = 0;
        for (bit, modifier) in [(4, SHIFT), (8, ALT), (16, CTRL)] {
            if button & bit != 0 {
                mods |= modifier;
            }
        }
        let base = button & 0xc3;
        let (button, action) = if (64..=67).contains(&base) {
            ((4 + base - 64) as u8, 0)
        } else {
            (
                match base & 3 {
                    0 => 1,
                    1 => 3,
                    2 => 2,
                    _ => 0,
                },
                if button & 32 != 0 {
                    2
                } else {
                    u8::from(final_byte != b'M')
                },
            )
        };
        return Some(Event::Mouse {
            button,
            action,
            mods,
            x: fields[1].saturating_sub(1).max(0) as u16,
            y: fields[2].saturating_sub(1).max(0) as u16,
        });
    }
    let mut nums = [0i32; 8];
    let mut subs = [[0i32; 3]; 8];
    let (mut count, mut sub, mut any) = (0usize, 0usize, false);
    for &byte in params {
        if count >= 8 {
            break;
        }
        if byte.is_ascii_digit() {
            any = true;
            digit(
                if sub == 0 {
                    &mut nums[count]
                } else {
                    &mut subs[count][sub]
                },
                byte,
            );
        } else if byte == b':' {
            sub = (sub + 1).min(2);
        } else if byte == b';' {
            count += 1;
            sub = 0;
        }
    }
    if any {
        count += 1;
    }
    let mod_param = if count >= 2 { nums[1] } else { 1 };
    let mods = decode_mods(mod_param.saturating_sub(1).max(0));
    let action = if count >= 2 {
        match subs[1][1] {
            3 => 0,
            2 => 2,
            _ => 1,
        }
    } else {
        1
    };
    if final_byte == b'u' {
        let cp = nums[0] as u32;
        let shifted = subs[0][1] as u32;
        let layout = subs[0][2] as u32;
        let out = if count >= 3 && nums[2] != 0 {
            nums[2] as u32
        } else if mods & (CTRL | SUPER) == 0 && cp >= 32 && cp != 127 {
            if mods & SHIFT != 0 && shifted != 0 {
                shifted
            } else {
                cp
            }
        } else {
            0
        };
        return Some(key_event(
            kitty_key(if layout != 0 { layout } else { cp }),
            mods,
            action,
            &if out == 0 { Vec::new() } else { encode_cp(out) },
            cp,
        ));
    }
    let (key, mods) = match final_byte {
        b'~' => (tilde_key(nums[0]), mods),
        b'Z' => (GHOSTTY_KEY_TAB, mods | SHIFT),
        byte => (final_key(byte), mods),
    };
    if key != GHOSTTY_KEY_UNIDENTIFIED || final_byte == b'~' {
        Some(key_event(key, mods, action, &[], 0))
    } else {
        None
    }
}

/// Parse one event without mutating the buffer. State changes are returned
/// separately so callbacks never borrow storage that the parser then moves.
fn parse_one(bytes: &[u8], in_paste: bool) -> Option<(usize, bool, Option<Event>)> {
    if bytes.is_empty() {
        return None;
    }
    if in_paste {
        let index = bytes.windows(6).position(|window| window == b"\x1b[201~")?;
        return Some((
            index + 6,
            false,
            Some(Event::Paste(bytes[..index].to_vec())),
        ));
    }
    let mut prefix = 0;
    while bytes.get(prefix) == Some(&0x1b) {
        let next = *bytes.get(prefix + 1)?;
        if next == b'[' {
            let end = prefix
                + 2
                + bytes[prefix + 2..]
                    .iter()
                    .position(|byte| (0x40..=0x7e).contains(byte))?;
            let params = &bytes[prefix + 2..end];
            if bytes[end] == b'~' && params == b"200" {
                // Legacy alt's captured tail has private paste state in C.
                return Some((end + 1, prefix == 0, None));
            }
            let mut event = csi(params, bytes[end]);
            if prefix != 0
                && let Some(Event::Key { mods, .. }) = &mut event
            {
                *mods |= ALT;
            }
            return Some((end + 1, false, event));
        }
        if next == b'O' {
            let final_byte = *bytes.get(prefix + 2)?;
            let key = final_key(final_byte);
            let event = (key != GHOSTTY_KEY_UNIDENTIFIED)
                .then(|| key_event(key, if prefix == 0 { 0 } else { ALT }, 1, &[], 0));
            return Some((prefix + 3, false, event));
        }
        prefix += 1;
    }
    let c = bytes[prefix];
    let (length, mut event) = match c {
        b'\r' | b'\n' => (1, key_event(GHOSTTY_KEY_ENTER, 0, 1, b"\r", 13)),
        b'\t' => (1, key_event(GHOSTTY_KEY_TAB, 0, 1, b"\t", 9)),
        8 | 127 => (1, key_event(GHOSTTY_KEY_BACKSPACE, 0, 1, &[], c as u32)),
        0 => (1, key_event(GHOSTTY_KEY_SPACE, CTRL, 1, &[], 32)),
        1..=31 => {
            let key = match c {
                0x1c => GHOSTTY_KEY_BACKSLASH,
                0x1d => GHOSTTY_KEY_BRACKET_RIGHT,
                0x1e => GHOSTTY_KEY_DIGIT_6,
                0x1f => GHOSTTY_KEY_MINUS,
                _ => GHOSTTY_KEY_A + (c - 1) as i32,
            };
            (
                1,
                key_event(
                    key,
                    CTRL,
                    1,
                    &[],
                    if c < 0x1b { (b'a' + c - 1) as u32 } else { 0 },
                ),
            )
        }
        _ => {
            let length = if c < 128 {
                1
            } else if c & 0xe0 == 0xc0 {
                2
            } else if c & 0xf0 == 0xe0 {
                3
            } else if c & 0xf8 == 0xf0 {
                4
            } else {
                1
            };
            if bytes.len() - prefix < length {
                return None;
            }
            let text = &bytes[prefix..prefix + length];
            let mut cp = match length {
                1 => c as u32,
                2 => (c & 31) as u32,
                3 => (c & 15) as u32,
                _ => (c & 7) as u32,
            };
            for &byte in &text[1..] {
                cp = (cp << 6) | (byte & 63) as u32;
            }
            let key = if cp < 128 {
                ascii_key(cp as u8)
            } else {
                GHOSTTY_KEY_UNIDENTIFIED
            };
            let mut mods = if (65..=90).contains(&cp) { SHIFT } else { 0 };
            let mut unshifted = if (65..=90).contains(&cp) { cp + 32 } else { cp };
            if let Some((_, base)) = SHIFT_PAIRS
                .iter()
                .find(|(shifted, _)| *shifted as u32 == cp)
            {
                mods |= SHIFT;
                unshifted = *base as u32;
            }
            (length, key_event(key, mods, 1, text, unshifted))
        }
    };
    if prefix != 0
        && let Event::Key { mods, .. } = &mut event
    {
        *mods |= ALT;
    }
    Some((prefix + length, false, Some(event)))
}

#[derive(Debug, Default)]
pub struct Parser {
    buffer: Vec<u8>,
    in_paste: bool,
}
impl Parser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut used = 0;
        while let Some((length, in_paste, event)) = parse_one(&self.buffer[used..], self.in_paste) {
            used += length;
            self.in_paste = in_paste;
            if let Some(event) = event {
                events.push(event);
            }
        }
        self.buffer.drain(..used);
        events
    }
    pub fn pending(&self) -> bool {
        !self.buffer.is_empty()
    }
    pub fn timeout(&mut self) -> Vec<Event> {
        if self.buffer.first() == Some(&0x1b) {
            self.buffer.remove(0);
            let mut events = vec![key_event(GHOSTTY_KEY_ESCAPE, 0, 1, &[], 27)];
            events.extend(self.feed(&[]));
            events
        } else {
            self.buffer.clear();
            Vec::new()
        }
    }
}

fn key_name(key: GhosttyKey) -> String {
    let name = match key {
        GHOSTTY_KEY_ARROW_UP => "ARROW_UP",
        GHOSTTY_KEY_ARROW_DOWN => "ARROW_DOWN",
        GHOSTTY_KEY_ARROW_LEFT => "ARROW_LEFT",
        GHOSTTY_KEY_ARROW_RIGHT => "ARROW_RIGHT",
        GHOSTTY_KEY_ENTER => "ENTER",
        GHOSTTY_KEY_TAB => "TAB",
        GHOSTTY_KEY_ESCAPE => "ESCAPE",
        GHOSTTY_KEY_BACKSPACE => "BACKSPACE",
        GHOSTTY_KEY_SPACE => "SPACE",
        GHOSTTY_KEY_HOME => "HOME",
        GHOSTTY_KEY_END => "END",
        GHOSTTY_KEY_INSERT => "INSERT",
        GHOSTTY_KEY_DELETE => "DELETE",
        GHOSTTY_KEY_PAGE_UP => "PAGE_UP",
        GHOSTTY_KEY_PAGE_DOWN => "PAGE_DOWN",
        GHOSTTY_KEY_UNIDENTIFIED => "UNIDENTIFIED",
        _ => "",
    };
    if !name.is_empty() {
        name.into()
    } else if (GHOSTTY_KEY_A..=GHOSTTY_KEY_Z).contains(&key) {
        ((b'a' + (key - GHOSTTY_KEY_A) as u8) as char).to_string()
    } else if (GHOSTTY_KEY_DIGIT_0..=GHOSTTY_KEY_DIGIT_9).contains(&key) {
        ((b'0' + (key - GHOSTTY_KEY_DIGIT_0) as u8) as char).to_string()
    } else if (GHOSTTY_KEY_F1..=GHOSTTY_KEY_F12).contains(&key) {
        format!("F{}", key - GHOSTTY_KEY_F1 + 1)
    } else {
        format!("key#{key}")
    }
}

impl Event {
    pub fn describe(&self) -> String {
        let mods = match self {
            Self::Key { mods, .. } | Self::Mouse { mods, .. } => *mods,
            _ => 0,
        };
        let mut modifiers = String::new();
        for (bit, label) in [(SHIFT, 'S'), (CTRL, 'C'), (ALT, 'A'), (SUPER, 'M')] {
            if mods & bit != 0 {
                modifiers.push(label);
            }
        }
        if modifiers.is_empty() {
            modifiers.push('-');
        }
        match self {
            Self::Key {
                key, action, text, ..
            } => {
                let action = match action {
                    1 => "press",
                    0 => "release",
                    _ => "repeat",
                };
                let text = if text.is_empty() {
                    String::new()
                } else {
                    format!(" text={}", String::from_utf8_lossy(text))
                };
                format!("key {} mods={modifiers} {action}{text}", key_name(*key))
            }
            Self::Mouse {
                button,
                action,
                x,
                y,
                ..
            } => {
                let action = match action {
                    0 => "press",
                    1 => "release",
                    _ => "motion",
                };
                format!("mouse btn={button} {action} at={x},{y} mods={modifiers}")
            }
            Self::Paste(bytes) => format!(
                "paste len={} text={}",
                bytes.len(),
                String::from_utf8_lossy(&bytes[..bytes.len().min(20)])
            ),
            Self::Focus(focused) => format!("focus {}", if *focused { "in" } else { "out" }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_read_boundary_preserves_events() {
        for sequence in [
            b"\x1b[97:65;2u".as_slice(),
            b"\x1b[200~hello\x1b[201~",
            b"\xf0\x9f\x92\x80",
            b"\x1b[<32;3;3M",
            b"\x1ba",
        ] {
            let expected = Parser::default().feed(sequence);
            for boundary in 0..=sequence.len() {
                let mut parser = Parser::default();
                let mut actual = parser.feed(&sequence[..boundary]);
                actual.extend(parser.feed(&sequence[boundary..]));
                assert_eq!(actual, expected, "boundary {boundary}: {sequence:?}");
            }
        }
    }

    #[test]
    fn legacy_alt_prefix_is_bounded_without_recursion() {
        let mut parser = Parser::default();
        let mut bytes = vec![0x1b; 10000];
        bytes.push(b'a');
        assert_eq!(
            parser.feed(&bytes)[0].describe(),
            "key a mods=A press text=a"
        );
        assert!(!parser.pending());
    }

    #[test]
    fn oversized_parameters_and_shifted_punctuation() {
        let mut parser = Parser::default();
        assert_eq!(
            parser.feed(b"?")[0],
            key_event(GHOSTTY_KEY_SLASH, SHIFT, 1, b"?", b'/' as u32)
        );
        assert_eq!(parser.feed(b"\x1b[<0;999999999999999;1M").len(), 1);
        parser.feed(b"\x1b[");
        assert_eq!(parser.timeout()[0].describe(), "key ESCAPE mods=- press");
    }
}
