//! Bounded shader bytecode with slosh's wrapping int32 arithmetic.
use std::fmt;

pub const DEP_POS: u32 = 1;
pub const DEP_CURSOR: u32 = 2;
pub const DEP_TIME: u32 = 4;
pub const DEP_FOCUS: u32 = 8;
pub const DEP_SIZE: u32 = 16;
pub const DEP_SCROLL: u32 = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Environment {
    pub x: i32,
    pub y: i32,
    pub cols: i32,
    pub rows: i32,
    pub curx: i32,
    pub cury: i32,
    pub cursor: i32,
    pub focused: i32,
    pub t: i64,
    pub since: i64,
    pub above: i32,
    pub below: i32,
}

const VARIABLES: [(&str, u32); 12] = [
    ("x", DEP_POS),
    ("y", DEP_POS),
    ("cols", DEP_SIZE),
    ("rows", DEP_SIZE),
    ("curx", DEP_CURSOR),
    ("cury", DEP_CURSOR),
    ("cursor", DEP_CURSOR),
    ("focused", DEP_FOCUS),
    ("t", DEP_TIME),
    ("since", DEP_TIME),
    ("above", DEP_SCROLL),
    ("below", DEP_SCROLL),
];

#[derive(Clone, Copy, Debug)]
enum Op {
    Push(i32),
    Variable(usize),
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    And,
    Or,
    Not,
    BitAnd,
    BitXor,
    BitOr,
    BitNot,
    Shl,
    Shr,
    Min,
    Max,
    Abs,
    Clamp,
    Dist,
    Sin,
    Cos,
    Select,
}
impl Op {
    fn arity(self) -> usize {
        match self {
            Self::Push(_) | Self::Variable(_) => 0,
            Self::Neg | Self::Not | Self::BitNot | Self::Abs | Self::Sin | Self::Cos => 1,
            Self::Clamp | Self::Select => 3,
            Self::Dist => 4,
            _ => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: &'static str,
    pub offset: Option<usize>,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)?;
        if let Some(offset) = self.offset {
            write!(f, " at offset {offset}")?;
        }
        Ok(())
    }
}
impl std::error::Error for Error {}

struct Parser<'a> {
    source: &'a [u8],
    offset: usize,
    code: Vec<Op>,
    deps: u32,
}
impl Parser<'_> {
    fn peek(&self) -> u8 {
        self.source.get(self.offset).copied().unwrap_or(0)
    }
    fn skip(&mut self) {
        while matches!(self.peek(), b' ' | b'\t' | b'\n' | b'\r') {
            self.offset += 1;
        }
    }
    fn error(&self, message: &'static str) -> Error {
        Error {
            message,
            offset: Some(self.offset),
        }
    }
    fn emit(&mut self, op: Op) -> Result<(), Error> {
        if self.code.len() == 256 {
            return Err(self.error("expression too long"));
        }
        self.code.push(op);
        Ok(())
    }
    fn expect(&mut self, byte: u8, message: &'static str) -> Result<(), Error> {
        self.skip();
        if self.peek() != byte {
            return Err(self.error(message));
        }
        self.offset += 1;
        Ok(())
    }
    fn primary(&mut self, depth: usize) -> Result<(), Error> {
        self.skip();
        if depth > 256 {
            return Err(self.error("expression nested too deeply"));
        }
        match self.peek() {
            b'(' => {
                self.offset += 1;
                self.expression(1, depth + 1)?;
                self.expect(b')', "expected )")
            }
            b'-' | b'!' | b'~' => {
                let op = match self.peek() {
                    b'-' => Op::Neg,
                    b'!' => Op::Not,
                    _ => Op::BitNot,
                };
                self.offset += 1;
                self.primary(depth + 1)?;
                self.emit(op)
            }
            b'0'..=b'9' => {
                let mut value = 0i32;
                while self.peek().is_ascii_digit() {
                    value = value
                        .saturating_mul(10)
                        .saturating_add((self.peek() - b'0') as i32);
                    self.offset += 1;
                }
                self.emit(Op::Push(value))
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = self.offset;
                while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
                    self.offset += 1;
                }
                if self.offset - start >= 32 {
                    return Err(self.error("name too long"));
                }
                let name = &self.source[start..self.offset];
                self.skip();
                if self.peek() == b'(' {
                    self.offset += 1;
                    let op = match name {
                        b"min" => Op::Min,
                        b"max" => Op::Max,
                        b"abs" => Op::Abs,
                        b"clamp" => Op::Clamp,
                        b"dist" => Op::Dist,
                        b"sin" => Op::Sin,
                        b"cos" => Op::Cos,
                        _ => return Err(self.error("unknown function")),
                    };
                    for index in 0..op.arity() {
                        if index != 0 {
                            self.expect(b',', "expected ,")?;
                        }
                        self.expression(1, depth + 1)?;
                    }
                    self.expect(b')', "expected )")?;
                    self.emit(op)
                } else if name == b"PI" {
                    self.emit(Op::Push(180))
                } else if name == b"TAU" {
                    self.emit(Op::Push(360))
                } else if let Some(index) = VARIABLES
                    .iter()
                    .position(|(text, _)| text.as_bytes() == name)
                {
                    self.deps |= VARIABLES[index].1;
                    self.emit(Op::Variable(index))
                } else {
                    Err(self.error("unknown name"))
                }
            }
            _ => Err(self.error("expected a value")),
        }
    }

    fn operator(&self) -> Option<(u8, usize, Op)> {
        let rest = self.source.get(self.offset..).unwrap_or_default();
        for (text, precedence, op) in [
            ("<<", 8, Op::Shl),
            (">>", 8, Op::Shr),
            ("<=", 4, Op::Le),
            (">=", 4, Op::Ge),
            ("==", 4, Op::Eq),
            ("!=", 4, Op::Ne),
            ("&&", 3, Op::And),
            ("||", 2, Op::Or),
        ] {
            if rest.starts_with(text.as_bytes()) {
                return Some((precedence, 2, op));
            }
        }
        let (precedence, op) = match self.peek() {
            b'*' => (10, Op::Mul),
            b'/' => (10, Op::Div),
            b'%' => (10, Op::Mod),
            b'+' => (9, Op::Add),
            b'-' => (9, Op::Sub),
            b'&' => (7, Op::BitAnd),
            b'^' => (6, Op::BitXor),
            b'|' => (5, Op::BitOr),
            b'<' => (4, Op::Lt),
            b'>' => (4, Op::Gt),
            _ => return None,
        };
        Some((precedence, 1, op))
    }

    fn expression(&mut self, min: u8, depth: usize) -> Result<(), Error> {
        self.primary(depth)?;
        loop {
            self.skip();
            let Some((precedence, length, op)) = self.operator() else {
                break;
            };
            if precedence < min {
                break;
            }
            self.offset += length;
            self.expression(precedence + 1, depth + 1)?;
            self.emit(op)?;
        }
        self.skip();
        if min == 1 && self.peek() == b'?' {
            self.offset += 1;
            self.expression(1, depth + 1)?;
            self.expect(b':', "expected :")?;
            self.expression(1, depth + 1)?;
            self.emit(Op::Select)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Program {
    source: String,
    code: Vec<Op>,
    deps: u32,
    constant: Option<i32>,
    map: Vec<u8>,
    key: Option<Environment>,
}

impl Program {
    pub fn compile(source: &str) -> Result<Self, Error> {
        if source.is_empty() {
            return Err(Error {
                message: "empty expression",
                offset: None,
            });
        }
        let mut parser = Parser {
            source: source.as_bytes(),
            offset: 0,
            code: Vec::new(),
            deps: 0,
        };
        parser.expression(1, 0)?;
        parser.skip();
        if parser.peek() != 0 {
            return Err(parser.error("unexpected trailing input"));
        }
        let mut stack = 0usize;
        for op in &parser.code {
            let arity = op.arity();
            if stack < arity {
                return Err(parser.error("malformed expression"));
            }
            stack = stack - arity + 1;
            if stack >= 31 {
                return Err(parser.error("malformed expression"));
            }
        }
        if stack != 1 {
            return Err(parser.error("malformed expression"));
        }
        let mut program = Self {
            source: source.into(),
            code: parser.code,
            deps: parser.deps,
            constant: None,
            map: Vec::new(),
            key: None,
        };
        if program.deps == 0 {
            program.constant = Some(program.run(&Environment::default()));
        }
        Ok(program)
    }

    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn deps(&self) -> u32 {
        self.deps
    }
    pub fn constant(&self) -> Option<i32> {
        self.constant
    }
    pub fn eval(&self, env: &Environment) -> i32 {
        self.constant.unwrap_or_else(|| self.run(env))
    }

    fn run(&self, env: &Environment) -> i32 {
        let vars = [
            env.x,
            env.y,
            env.cols,
            env.rows,
            env.curx,
            env.cury,
            env.cursor,
            env.focused,
            (env.t & 0x7fffffff) as i32,
            env.since.clamp(0, i32::MAX as i64) as i32,
            env.above,
            env.below,
        ];
        let mut stack = [0i32; 32];
        let mut count = 0;
        for &op in &self.code {
            let arity = op.arity();
            let start = count - arity;
            let args = &stack[start..count];
            let value = match op {
                Op::Push(value) => value,
                Op::Variable(index) => vars[index],
                Op::Add => args[0].wrapping_add(args[1]),
                Op::Sub => args[0].wrapping_sub(args[1]),
                Op::Mul => args[0].wrapping_mul(args[1]),
                Op::Div => {
                    if args[1] == 0 {
                        0
                    } else {
                        args[0].wrapping_div(args[1])
                    }
                }
                Op::Mod => {
                    if args[1] == 0 {
                        0
                    } else {
                        args[0].wrapping_rem(args[1])
                    }
                }
                Op::Neg => args[0].wrapping_neg(),
                Op::Abs => args[0].wrapping_abs(),
                Op::Not => (args[0] == 0) as i32,
                Op::BitNot => !args[0],
                Op::Lt => (args[0] < args[1]) as i32,
                Op::Gt => (args[0] > args[1]) as i32,
                Op::Le => (args[0] <= args[1]) as i32,
                Op::Ge => (args[0] >= args[1]) as i32,
                Op::Eq => (args[0] == args[1]) as i32,
                Op::Ne => (args[0] != args[1]) as i32,
                Op::And => (args[0] != 0 && args[1] != 0) as i32,
                Op::Or => (args[0] != 0 || args[1] != 0) as i32,
                Op::BitAnd => args[0] & args[1],
                Op::BitOr => args[0] | args[1],
                Op::BitXor => args[0] ^ args[1],
                Op::Shl => args[0].wrapping_shl(args[1] as u32),
                Op::Shr => args[0].wrapping_shr(args[1] as u32),
                Op::Min => args[0].min(args[1]),
                Op::Max => args[0].max(args[1]),
                Op::Clamp => {
                    if args[0] < args[1] {
                        args[1]
                    } else if args[0] > args[2] {
                        args[2]
                    } else {
                        args[0]
                    }
                }
                Op::Select => {
                    if args[0] != 0 {
                        args[1]
                    } else {
                        args[2]
                    }
                }
                Op::Sin => sin(args[0]),
                Op::Cos => sin(args[0].wrapping_add(90)),
                Op::Dist => {
                    let dx = (args[0] as i64 - args[2] as i64).clamp(-65536, 65536);
                    let dy = ((args[1] as i64 - args[3] as i64) * 2).clamp(-65536, 65536);
                    (dx * dx + dy * dy).isqrt().min(4096) as i32
                }
            };
            stack[start] = value;
            count = start + 1;
        }
        stack[0]
    }

    /// Time-dependent programs have no reusable map. Other maps invalidate only
    /// for dimensions and the environment dependencies detected by the compiler.
    pub fn amount_map(&mut self, env: &Environment) -> Option<&[u8]> {
        if self.deps & DEP_TIME != 0 || env.cols <= 0 || env.rows <= 0 {
            return None;
        }
        let same = self.key.is_some_and(|key| {
            key.cols == env.cols
                && key.rows == env.rows
                && (self.deps & DEP_CURSOR == 0
                    || (key.curx, key.cury, key.cursor) == (env.curx, env.cury, env.cursor))
                && (self.deps & DEP_FOCUS == 0 || key.focused == env.focused)
                && (self.deps & DEP_SCROLL == 0 || (key.above, key.below) == (env.above, env.below))
        });
        if !same {
            let cells = (env.cols as usize).checked_mul(env.rows as usize)?;
            self.map
                .try_reserve(cells.saturating_sub(self.map.len()))
                .ok()?;
            self.map.resize(cells, 0);
            let mut point = *env;
            for y in 0..env.rows {
                point.y = y;
                for x in 0..env.cols {
                    point.x = x;
                    self.map[y as usize * env.cols as usize + x as usize] =
                        self.eval(&point).clamp(0, 255) as u8;
                }
            }
            self.key = Some(*env);
        }
        Some(&self.map)
    }

    pub(crate) fn cached_amount_map(&self) -> &[u8] {
        &self.map
    }
}

const SIN_QUARTER: [i16; 91] = [
    0, 4, 9, 13, 18, 22, 27, 31, 35, 40, 44, 49, 53, 57, 62, 66, 70, 75, 79, 83, 87, 91, 96, 100,
    104, 108, 112, 116, 120, 124, 127, 131, 135, 139, 143, 146, 150, 153, 157, 160, 164, 167, 171,
    174, 177, 180, 183, 186, 190, 192, 195, 198, 201, 204, 206, 209, 211, 214, 216, 219, 221, 223,
    225, 227, 229, 231, 233, 235, 236, 238, 240, 241, 243, 244, 245, 246, 247, 248, 249, 250, 251,
    252, 253, 253, 254, 254, 254, 255, 255, 255, 255,
];

pub fn sin(degrees: i32) -> i32 {
    let degree = degrees.rem_euclid(360) as usize;
    match degree {
        0..=90 => SIN_QUARTER[degree] as i32,
        91..=180 => SIN_QUARTER[180 - degree] as i32,
        181..=270 => -(SIN_QUARTER[degree - 180] as i32),
        _ => -(SIN_QUARTER[360 - degree] as i32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_arithmetic_and_precedence() {
        for (source, value) in [
            ("1+2*3", 7),
            ("12 & 8 == 8", 1),
            ("1 | 6 & 4", 5),
            ("1 << 33", 2),
            ("-8 >> 1", -4),
            ("2147483647 + 1", i32::MIN),
            ("(2147483647 + 1) / -1", i32::MIN),
            ("(2147483647 + 1) % -1", 0),
            ("0 ? 1/0 : 7", 7),
            ("sin(PI/6)", 127),
            ("dist(0,0,0,2)", 4),
        ] {
            assert_eq!(
                Program::compile(source)
                    .unwrap()
                    .eval(&Environment::default()),
                value,
                "{source}"
            );
        }
    }

    #[test]
    fn scroll_focus_and_cursor_invalidate_maps() {
        let mut program = Program::compile("x + above + focused + curx").unwrap();
        let mut env = Environment {
            cols: 4,
            rows: 2,
            ..Default::default()
        };
        assert_eq!(program.amount_map(&env).unwrap()[3], 3);
        env.above = 4;
        env.focused = 1;
        env.curx = 2;
        assert_eq!(program.amount_map(&env).unwrap()[3], 10);
        env.cols = 2;
        assert_eq!(program.amount_map(&env).unwrap(), &[7, 8, 7, 8]);
        assert_eq!(
            Program::compile("since").unwrap().eval(&Environment {
                since: i64::MAX,
                ..env
            }),
            i32::MAX
        );
        assert!(Program::compile("t").unwrap().amount_map(&env).is_none());
    }

    #[test]
    fn untrusted_expressions_are_bounded() {
        for source in [
            "1+".repeat(200),
            "-".repeat(10000),
            "(".repeat(300),
            "1 ? 2".into(),
            "unknown(1)".into(),
        ] {
            assert!(Program::compile(&source).is_err());
        }
    }
}
