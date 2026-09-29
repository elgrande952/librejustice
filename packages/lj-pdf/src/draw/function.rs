//! PDF functions: sampled, exponential, stitching and PostScript calculator.

use crate::objects::{
    PdfDictionary, PdfError, PdfFile, PdfObject, PdfResult, PdfValue, decode_stream,
};

/// Deeper stitching than this is a loop between objects or a hostile file.
const MAX_DEPTH: usize = 8;
const MAX_SAMPLES: usize = 1 << 22;
const MAX_INPUTS: usize = 8;
const MAX_STACK: usize = 100;
const MAX_PROGRAM: usize = 1 << 16;

#[derive(Clone, Debug)]
pub struct Function {
    domain: Vec<[f64; 2]>,
    range: Option<Vec<[f64; 2]>>,
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    Sampled {
        size: Vec<usize>,
        encode: Vec<[f64; 2]>,
        outputs: usize,
        /// Decoded samples, the first input varying fastest.
        samples: Vec<f32>,
    },
    Exponential {
        c0: Vec<f64>,
        c1: Vec<f64>,
        n: f64,
    },
    Stitching {
        functions: Vec<Function>,
        bounds: Vec<f64>,
        encode: Vec<[f64; 2]>,
    },
    Calculator(Vec<Instr>),
}

pub(super) fn corrupt(message: &str) -> PdfError {
    PdfError::Corrupt(message.to_string())
}

pub(super) fn number(file: &PdfFile, value: &PdfValue) -> Option<f64> {
    file.resolve(value).ok()?.as_number()
}

pub(super) fn numbers(file: &PdfFile, value: Option<&PdfValue>) -> Option<Vec<f64>> {
    let items = file.resolve(value?).ok()?.as_array()?;
    items.iter().map(|item| number(file, item)).collect()
}

fn pairs(values: &[f64]) -> Vec<[f64; 2]> {
    values.as_chunks::<2>().0.to_vec()
}

pub(super) fn interpolate(x: f64, [x0, x1]: [f64; 2], [y0, y1]: [f64; 2]) -> f64 {
    match x1 == x0 {
        true => y0,
        false => y0 + (x - x0) * (y1 - y0) / (x1 - x0),
    }
}

pub(super) fn clip(x: f64, [low, high]: [f64; 2]) -> f64 {
    if x.is_nan() {
        low
    } else {
        x.max(low.min(high)).min(high.max(low))
    }
}

/// The dictionary of a function and, for types 0 and 4, its decoded stream.
fn body<'a>(
    file: &'a PdfFile,
    value: &'a PdfValue,
) -> PdfResult<(&'a PdfDictionary, Option<Vec<u8>>)> {
    match value {
        PdfValue::Reference(object_ref) => match file.get_object(*object_ref)? {
            PdfObject::Stream(stream) => Ok((&stream.dict, Some(decode_stream(stream)?))),
            PdfObject::Value(PdfValue::Dictionary(dict)) => Ok((dict, None)),
            PdfObject::Value(_) => Err(corrupt("function is neither a dictionary nor a stream")),
        },
        PdfValue::Dictionary(dict) => Ok((dict, None)),
        _ => Err(corrupt("function is neither a dictionary nor a stream")),
    }
}

impl Function {
    pub fn parse(file: &PdfFile, value: &PdfValue) -> PdfResult<Function> {
        Self::parse_at(file, value, 0)
    }

    /// From `c0` at 0 to `c1` at 1, in a straight line.
    pub fn linear(c0: Vec<f64>, c1: Vec<f64>) -> Function {
        Function {
            domain: vec![[0.0, 1.0]],
            range: None,
            kind: Kind::Exponential { c0, c1, n: 1.0 },
        }
    }

    /// One function, or an array of one-output functions whose outputs follow each other.
    pub fn parse_list(file: &PdfFile, value: &PdfValue) -> PdfResult<Vec<Function>> {
        let array = match value {
            PdfValue::Reference(object_ref) => match file.get_object(*object_ref)? {
                PdfObject::Value(PdfValue::Array(items)) => Some(items),
                _ => None,
            },
            PdfValue::Array(items) => Some(items),
            _ => None,
        };
        match array {
            Some(items) => items.iter().map(|item| Self::parse(file, item)).collect(),
            None => Ok(vec![Self::parse(file, value)?]),
        }
    }

    fn parse_at(file: &PdfFile, value: &PdfValue, depth: usize) -> PdfResult<Function> {
        if depth > MAX_DEPTH {
            return Err(corrupt("functions nest too deep"));
        }
        let (dict, data) = body(file, value)?;
        let domain = pairs(
            &numbers(file, dict.get("Domain")).ok_or_else(|| corrupt("function has no Domain"))?,
        );
        if domain.is_empty() || domain.len() > MAX_INPUTS {
            return Err(corrupt("function Domain is out of bounds"));
        }
        let range = numbers(file, dict.get("Range"))
            .map(|r| pairs(&r))
            .filter(|r| !r.is_empty());
        let kind = match dict
            .get("FunctionType")
            .and_then(|v| number(file, v))
            .map(|n| n as i64)
        {
            Some(0) => {
                let range = range
                    .as_ref()
                    .ok_or_else(|| corrupt("sampled function has no Range"))?;
                let data = data.ok_or_else(|| corrupt("sampled function is not a stream"))?;
                sampled(file, dict, &domain, range, &data)?
            }
            Some(2) => {
                let c0 = numbers(file, dict.get("C0")).unwrap_or_else(|| vec![0.0]);
                let c1 = numbers(file, dict.get("C1")).unwrap_or_else(|| vec![1.0]);
                if c0.len() != c1.len() {
                    return Err(corrupt("exponential function C0 and C1 differ in length"));
                }
                let n = dict
                    .get("N")
                    .and_then(|v| number(file, v))
                    .ok_or_else(|| corrupt("exponential function has no N"))?;
                Kind::Exponential { c0, c1, n }
            }
            Some(3) => {
                let items = dict
                    .get("Functions")
                    .and_then(|v| file.resolve(v).ok())
                    .and_then(PdfValue::as_array)
                    .ok_or_else(|| corrupt("stitching function has no Functions"))?;
                let functions = items
                    .iter()
                    .map(|item| Self::parse_at(file, item, depth + 1))
                    .collect::<PdfResult<Vec<_>>>()?;
                let bounds = numbers(file, dict.get("Bounds")).unwrap_or_default();
                let encode = pairs(&numbers(file, dict.get("Encode")).unwrap_or_default());
                if functions.is_empty()
                    || bounds.len() + 1 != functions.len()
                    || encode.len() != functions.len()
                {
                    return Err(corrupt("stitching function arrays disagree"));
                }
                Kind::Stitching {
                    functions,
                    bounds,
                    encode,
                }
            }
            Some(4) => {
                let data = data.ok_or_else(|| corrupt("calculator function is not a stream"))?;
                Kind::Calculator(compile(&data)?)
            }
            _ => return Err(PdfError::Unsupported("function type".to_string())),
        };
        Ok(Function {
            domain,
            range,
            kind,
        })
    }

    /// Appends the outputs at `input` to `out`, clipped to the range; a calculator that fails yields the bottom of its range.
    pub fn eval(&self, input: &[f64], out: &mut Vec<f64>) {
        let x: Vec<f64> = self
            .domain
            .iter()
            .enumerate()
            .map(|(i, d)| clip(input.get(i).copied().unwrap_or(d[0]), *d))
            .collect();
        let start = out.len();
        match &self.kind {
            Kind::Sampled {
                size,
                encode,
                outputs,
                samples,
            } => sample(&x, &self.domain, size, encode, *outputs, samples, out),
            Kind::Exponential { c0, c1, n } => {
                let t = x[0].powf(*n);
                let t = if t.is_finite() { t } else { 0.0 };
                out.extend(c0.iter().zip(c1).map(|(a, b)| a + t * (b - a)));
            }
            Kind::Stitching {
                functions,
                bounds,
                encode,
            } => {
                let x = x[0];
                let [d0, d1] = self.domain[0];
                let i = bounds.iter().take_while(|b| x >= **b).count();
                let i = if i > 0 && bounds[0] == d0 && x == d0 {
                    0
                } else {
                    i
                };
                let low = if i == 0 { d0 } else { bounds[i - 1] };
                let high = if i == bounds.len() { d1 } else { bounds[i] };
                functions[i].eval(&[interpolate(x, [low, high], encode[i])], out);
            }
            Kind::Calculator(program) => {
                let n = self.range.as_ref().map_or(1, Vec::len);
                match run(program, &x) {
                    Some(stack) if stack.len() >= n => {
                        out.extend(stack[stack.len() - n..].iter().map(|v| v.number()));
                    }
                    _ => out.extend(self.range.iter().flatten().map(|r| r[0]).take(n)),
                }
            }
        }
        if let Some(range) = &self.range {
            for (value, r) in out[start..].iter_mut().zip(range) {
                *value = clip(*value, *r);
            }
        }
    }
}

/// The outputs of every function of `list` at `input`, one after the other.
pub fn eval_list(list: &[Function], input: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(4);
    for function in list {
        function.eval(input, &mut out);
    }
    out
}

fn sampled(
    file: &PdfFile,
    dict: &PdfDictionary,
    domain: &[[f64; 2]],
    range: &[[f64; 2]],
    data: &[u8],
) -> PdfResult<Kind> {
    let size: Vec<usize> = numbers(file, dict.get("Size"))
        .ok_or_else(|| corrupt("sampled function has no Size"))?
        .iter()
        .map(|s| *s as usize)
        .collect();
    if size.len() != domain.len() || size.contains(&0) {
        return Err(corrupt("sampled function Size disagrees with Domain"));
    }
    let bits = dict
        .get("BitsPerSample")
        .and_then(|v| number(file, v))
        .unwrap_or(8.0) as u32;
    if !matches!(bits, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32) {
        return Err(corrupt("sampled function BitsPerSample"));
    }
    let outputs = range.len();
    let count = size
        .iter()
        .try_fold(outputs, |acc, s| acc.checked_mul(*s))
        .filter(|n| *n <= MAX_SAMPLES)
        .ok_or_else(|| PdfError::Unsupported("sampled function too large".to_string()))?;
    let encode = numbers(file, dict.get("Encode"))
        .map(|e| pairs(&e))
        .filter(|e| e.len() == size.len())
        .unwrap_or_else(|| size.iter().map(|s| [0.0, (*s - 1) as f64]).collect());
    let decode = numbers(file, dict.get("Decode"))
        .map(|d| pairs(&d))
        .filter(|d| d.len() == outputs)
        .unwrap_or_else(|| range.to_vec());
    let max = ((1u64 << bits) - 1) as f64;
    let mut samples = Vec::with_capacity(count);
    let mut reader = Bits { data, position: 0 };
    for i in 0..count {
        let raw = f64::from(reader.read(bits).unwrap_or(0));
        samples.push(interpolate(raw, [0.0, max], decode[i % outputs]) as f32);
    }
    Ok(Kind::Sampled {
        size,
        encode,
        outputs,
        samples,
    })
}

/// A big-endian bit reader over packed samples.
pub(super) struct Bits<'a> {
    pub data: &'a [u8],
    pub position: usize,
}

impl Bits<'_> {
    pub fn read(&mut self, bits: u32) -> Option<u32> {
        if self.position + bits as usize > self.data.len() * 8 {
            self.position += bits as usize;
            return None;
        }
        let mut value: u64 = 0;
        let mut left = bits;
        while left > 0 {
            let byte = self.data[self.position / 8];
            let offset = (self.position % 8) as u32;
            let take = left.min(8 - offset);
            let chunk = (u32::from(byte) >> (8 - offset - take)) & ((1u32 << take) - 1);
            value = (value << take) | u64::from(chunk);
            left -= take;
            self.position += take as usize;
        }
        Some(value as u32)
    }

    pub fn align(&mut self) {
        self.position = self.position.div_ceil(8) * 8;
    }
}

fn sample(
    x: &[f64],
    domain: &[[f64; 2]],
    size: &[usize],
    encode: &[[f64; 2]],
    outputs: usize,
    samples: &[f32],
    out: &mut Vec<f64>,
) {
    let m = x.len();
    let mut low = [0usize; MAX_INPUTS];
    let mut frac = [0f64; MAX_INPUTS];
    let mut stride = [0usize; MAX_INPUTS];
    let mut s = outputs;
    for i in 0..m {
        let last = (size[i] - 1) as f64;
        let e = clip(interpolate(x[i], domain[i], encode[i]), [0.0, last]);
        let floor = e.floor().min((size[i].max(2) - 2) as f64).max(0.0);
        low[i] = floor as usize;
        frac[i] = if size[i] == 1 { 0.0 } else { e - floor };
        stride[i] = s;
        s *= size[i];
    }
    let start = out.len();
    out.resize(start + outputs, 0.0);
    for corner in 0..(1usize << m) {
        let mut weight = 1.0;
        let mut index = 0;
        for i in 0..m {
            let up = corner >> i & 1 == 1;
            let k = if up {
                (low[i] + 1).min(size[i] - 1)
            } else {
                low[i]
            };
            weight *= if up { frac[i] } else { 1.0 - frac[i] };
            index += k * stride[i];
        }
        if weight == 0.0 {
            continue;
        }
        for j in 0..outputs {
            out[start + j] += weight * f64::from(samples[index + j]);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Value {
    Int(i64),
    Real(f64),
    Bool(bool),
}

impl Value {
    fn number(self) -> f64 {
        match self {
            Value::Int(i) => i as f64,
            Value::Real(r) => r,
            Value::Bool(b) => f64::from(u8::from(b)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Abs,
    Add,
    And,
    Atan,
    Bitshift,
    Ceiling,
    Copy,
    Cos,
    Cvi,
    Cvr,
    Div,
    Dup,
    Eq,
    Exch,
    Exp,
    Floor,
    Ge,
    Gt,
    Idiv,
    Index,
    Le,
    Ln,
    Log,
    Lt,
    Mod,
    Mul,
    Ne,
    Neg,
    Not,
    Or,
    Pop,
    Roll,
    Round,
    Sin,
    Sqrt,
    Sub,
    Truncate,
    Xor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Instr {
    Push(Value),
    Op(Op),
    /// Skip that many instructions when the popped value is false.
    JumpUnless(usize),
    Jump(usize),
}

fn operator(word: &str) -> Option<Op> {
    Some(match word {
        "abs" => Op::Abs,
        "add" => Op::Add,
        "and" => Op::And,
        "atan" => Op::Atan,
        "bitshift" => Op::Bitshift,
        "ceiling" => Op::Ceiling,
        "copy" => Op::Copy,
        "cos" => Op::Cos,
        "cvi" => Op::Cvi,
        "cvr" => Op::Cvr,
        "div" => Op::Div,
        "dup" => Op::Dup,
        "eq" => Op::Eq,
        "exch" => Op::Exch,
        "exp" => Op::Exp,
        "floor" => Op::Floor,
        "ge" => Op::Ge,
        "gt" => Op::Gt,
        "idiv" => Op::Idiv,
        "index" => Op::Index,
        "le" => Op::Le,
        "ln" => Op::Ln,
        "log" => Op::Log,
        "lt" => Op::Lt,
        "mod" => Op::Mod,
        "mul" => Op::Mul,
        "ne" => Op::Ne,
        "neg" => Op::Neg,
        "not" => Op::Not,
        "or" => Op::Or,
        "pop" => Op::Pop,
        "roll" => Op::Roll,
        "round" => Op::Round,
        "sin" => Op::Sin,
        "sqrt" => Op::Sqrt,
        "sub" => Op::Sub,
        "truncate" => Op::Truncate,
        "xor" => Op::Xor,
        _ => return None,
    })
}

fn tokens(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let c = data[i];
        if c == b'%' {
            while i < data.len() && data[i] != b'\n' && data[i] != b'\r' {
                i += 1;
            }
        } else if c == b'{' || c == b'}' {
            out.push(&data[i..i + 1]);
            i += 1;
        } else if c.is_ascii_whitespace() || c == 0 {
            i += 1;
        } else {
            let start = i;
            while i < data.len()
                && !data[i].is_ascii_whitespace()
                && !matches!(data[i], b'{' | b'}' | b'%' | 0)
            {
                i += 1;
            }
            out.push(&data[start..i]);
        }
    }
    out
}

fn compile(data: &[u8]) -> PdfResult<Vec<Instr>> {
    let tokens = tokens(data);
    if tokens.len() > MAX_PROGRAM {
        return Err(PdfError::Unsupported(
            "calculator program too long".to_string(),
        ));
    }
    if tokens.first() != Some(&&b"{"[..]) {
        return Err(corrupt("calculator program does not open with a brace"));
    }
    let mut position = 1;
    block(&tokens, &mut position, 0)
}

/// The instructions up to the closing brace, which it consumes.
fn block(tokens: &[&[u8]], position: &mut usize, depth: usize) -> PdfResult<Vec<Instr>> {
    if depth > 32 {
        return Err(corrupt("calculator blocks nest too deep"));
    }
    let mut out = Vec::new();
    let mut pending: Vec<Vec<Instr>> = Vec::new();
    while let Some(token) = tokens.get(*position) {
        *position += 1;
        match *token {
            b"{" => pending.push(block(tokens, position, depth + 1)?),
            b"}" => return Ok(out),
            b"if" => {
                let then = pending
                    .pop()
                    .ok_or_else(|| corrupt("calculator if without a procedure"))?;
                pending.clear();
                out.push(Instr::JumpUnless(then.len()));
                out.extend(then);
            }
            b"ifelse" => {
                let otherwise = pending
                    .pop()
                    .ok_or_else(|| corrupt("calculator ifelse without procedures"))?;
                let then = pending
                    .pop()
                    .ok_or_else(|| corrupt("calculator ifelse without procedures"))?;
                pending.clear();
                out.push(Instr::JumpUnless(then.len() + 1));
                out.extend(then);
                out.push(Instr::Jump(otherwise.len()));
                out.extend(otherwise);
            }
            word => {
                pending.clear();
                let text = std::str::from_utf8(word).map_err(|_| corrupt("calculator token"))?;
                out.push(match text {
                    "true" => Instr::Push(Value::Bool(true)),
                    "false" => Instr::Push(Value::Bool(false)),
                    _ => match operator(text) {
                        Some(op) => Instr::Op(op),
                        None => match text.parse::<i64>() {
                            Ok(i) => Instr::Push(Value::Int(i)),
                            Err(_) => Instr::Push(Value::Real(
                                text.parse::<f64>()
                                    .map_err(|_| corrupt("calculator operator"))?,
                            )),
                        },
                    },
                });
            }
        }
    }
    Err(corrupt("calculator program does not close"))
}

fn run(program: &[Instr], input: &[f64]) -> Option<Vec<Value>> {
    let mut stack: Vec<Value> = input.iter().map(|x| Value::Real(*x)).collect();
    let mut pc = 0;
    while pc < program.len() {
        let instr = program[pc];
        pc += 1;
        match instr {
            Instr::Push(value) => stack.push(value),
            Instr::Jump(n) => pc += n,
            Instr::JumpUnless(n) => match stack.pop()? {
                Value::Bool(true) => {}
                Value::Bool(false) => pc += n,
                _ => return None,
            },
            Instr::Op(op) => apply(op, &mut stack)?,
        }
        if stack.len() > MAX_STACK {
            return None;
        }
    }
    Some(stack)
}

fn apply(op: Op, stack: &mut Vec<Value>) -> Option<()> {
    use Value::{Bool, Int, Real};
    let int = |v: Value| match v {
        Int(i) => Some(i),
        _ => None,
    };
    let real = |v: Value| match v {
        Bool(_) => None,
        v => Some(v.number()),
    };
    let rounded = |v: Value, f: fn(f64) -> f64| match v {
        Int(_) => Some(v),
        Real(r) => Some(Real(f(r))),
        Bool(_) => None,
    };
    let result = match op {
        Op::Dup => *stack.last()?,
        Op::Pop => {
            stack.pop()?;
            return Some(());
        }
        Op::Exch => {
            let n = stack.len();
            if n < 2 {
                return None;
            }
            stack.swap(n - 1, n - 2);
            return Some(());
        }
        Op::Copy => {
            let n = usize::try_from(int(stack.pop()?)?).ok()?;
            let len = stack.len();
            if n > len || len + n > MAX_STACK {
                return None;
            }
            stack.extend_from_within(len - n..);
            return Some(());
        }
        Op::Index => {
            let n = usize::try_from(int(stack.pop()?)?).ok()?;
            *stack.get(stack.len().checked_sub(n + 1)?)?
        }
        Op::Roll => {
            let j = int(stack.pop()?)?;
            let n = usize::try_from(int(stack.pop()?)?).ok()?;
            let len = stack.len();
            if n > len {
                return None;
            }
            if n > 0 {
                stack[len - n..].rotate_right(j.rem_euclid(n as i64) as usize);
            }
            return Some(());
        }
        Op::Abs => match stack.pop()? {
            Int(i) => i.checked_abs().map_or(Real((i as f64).abs()), Int),
            Real(r) => Real(r.abs()),
            Bool(_) => return None,
        },
        Op::Neg => match stack.pop()? {
            Int(i) => i.checked_neg().map_or(Real(-(i as f64)), Int),
            Real(r) => Real(-r),
            Bool(_) => return None,
        },
        Op::Ceiling => rounded(stack.pop()?, f64::ceil)?,
        Op::Floor => rounded(stack.pop()?, f64::floor)?,
        Op::Round => rounded(stack.pop()?, |r| (r + 0.5).floor())?,
        Op::Truncate => rounded(stack.pop()?, f64::trunc)?,
        Op::Cvi => Int(real(stack.pop()?)?.trunc() as i64),
        Op::Cvr => Real(real(stack.pop()?)?),
        Op::Sqrt => Real(real(stack.pop()?)?.max(0.0).sqrt()),
        Op::Sin => Real(real(stack.pop()?)?.to_radians().sin()),
        Op::Cos => Real(real(stack.pop()?)?.to_radians().cos()),
        Op::Ln => Real(real(stack.pop()?)?.ln()),
        Op::Log => Real(real(stack.pop()?)?.log10()),
        Op::Not => match stack.pop()? {
            Bool(b) => Bool(!b),
            Int(i) => Int(!i),
            Real(_) => return None,
        },
        _ => {
            let b = stack.pop()?;
            let a = stack.pop()?;
            binary(op, a, b)?
        }
    };
    stack.push(result);
    Some(())
}

fn binary(op: Op, a: Value, b: Value) -> Option<Value> {
    use Value::{Bool, Int, Real};
    let arith = |int: fn(i64, i64) -> Option<i64>, real: fn(f64, f64) -> f64| match (a, b) {
        (Int(x), Int(y)) => Some(int(x, y).map_or_else(|| Real(real(x as f64, y as f64)), Int)),
        (Bool(_), _) | (_, Bool(_)) => None,
        _ => Some(Real(real(a.number(), b.number()))),
    };
    let numeric = !matches!(a, Bool(_)) && !matches!(b, Bool(_));
    let compare = |f: fn(f64, f64) -> bool| numeric.then(|| Bool(f(a.number(), b.number())));
    let equal = a == b || (numeric && a.number() == b.number());
    Some(match op {
        Op::Add => arith(i64::checked_add, |x, y| x + y)?,
        Op::Sub => arith(i64::checked_sub, |x, y| x - y)?,
        Op::Mul => arith(i64::checked_mul, |x, y| x * y)?,
        Op::Div if numeric => Real(if b.number() == 0.0 {
            0.0
        } else {
            a.number() / b.number()
        }),
        Op::Idiv | Op::Mod | Op::Bitshift => {
            let (Int(x), Int(y)) = (a, b) else {
                return None;
            };
            match op {
                Op::Idiv => Int(x.checked_div(y)?),
                Op::Mod => Int(x.checked_rem(y)?),
                _ if y >= 0 => Int(x.checked_shl(y.min(63) as u32).unwrap_or(0)),
                _ => Int(x.checked_shr((-y).min(63) as u32).unwrap_or(0)),
            }
        }
        Op::Atan if numeric => {
            let angle = a.number().atan2(b.number()).to_degrees();
            Real(if angle < 0.0 { angle + 360.0 } else { angle })
        }
        Op::Exp if numeric => Real(a.number().powf(b.number())),
        Op::Eq => Bool(equal),
        Op::Ne => Bool(!equal),
        Op::Gt => compare(|x, y| x > y)?,
        Op::Ge => compare(|x, y| x >= y)?,
        Op::Lt => compare(|x, y| x < y)?,
        Op::Le => compare(|x, y| x <= y)?,
        Op::And | Op::Or | Op::Xor => match (a, b) {
            (Bool(x), Bool(y)) => Bool(match op {
                Op::And => x & y,
                Op::Or => x | y,
                _ => x ^ y,
            }),
            (Int(x), Int(y)) => Int(match op {
                Op::And => x & y,
                Op::Or => x | y,
                _ => x ^ y,
            }),
            _ => return None,
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calculator(program: &str, domain: usize, range: usize) -> Function {
        Function {
            domain: vec![[0.0, 1.0]; domain],
            range: Some(vec![[-1000.0, 1000.0]; range]),
            kind: Kind::Calculator(compile(program.as_bytes()).unwrap()),
        }
    }

    fn eval(f: &Function, input: &[f64]) -> Vec<f64> {
        let mut out = Vec::new();
        f.eval(input, &mut out);
        out
    }

    #[test]
    fn calculator_runs_arithmetic_and_branches() {
        let f = calculator("{ dup 0.5 gt { 2 mul } { 1 exch sub } ifelse }", 1, 1);
        assert_eq!(eval(&f, &[0.75]), vec![1.5]);
        assert_eq!(eval(&f, &[0.25]), vec![0.75]);
        let f = calculator("{ 360 mul sin 2 div 0.5 add }", 1, 1);
        assert!((eval(&f, &[0.25])[0] - 1.0).abs() < 1e-9);
        let f = calculator("{ 1 index 1 index add 3 1 roll }", 2, 3);
        assert_eq!(eval(&f, &[0.25, 0.5]), vec![0.75, 0.25, 0.5]);
        let f = calculator("{ pop 7 2 idiv 7 3 mod 1 3 bitshift }", 1, 3);
        assert_eq!(eval(&f, &[0.0]), vec![3.0, 1.0, 8.0]);
    }

    #[test]
    fn calculator_nests_an_if_inside_an_ifelse() {
        let f = calculator(
            "{ dup 0.5 lt { dup 0.25 lt { pop 0 } if } { pop 1 } ifelse }",
            1,
            1,
        );
        assert_eq!(eval(&f, &[0.1]), vec![0.0]);
        assert_eq!(eval(&f, &[0.3]), vec![0.3]);
        assert_eq!(eval(&f, &[0.9]), vec![1.0]);
    }

    #[test]
    fn a_failing_calculator_paints_the_bottom_of_its_range() {
        let f = Function {
            domain: vec![[0.0, 1.0]],
            range: Some(vec![[0.2, 1.0]]),
            kind: Kind::Calculator(compile(b"{ pop pop }").unwrap()),
        };
        assert_eq!(eval(&f, &[0.5]), vec![0.2]);
    }

    #[test]
    fn stitching_picks_the_interval_and_encodes_into_it() {
        let linear = |c0: f64, c1: f64| Function {
            domain: vec![[0.0, 1.0]],
            range: None,
            kind: Kind::Exponential {
                c0: vec![c0],
                c1: vec![c1],
                n: 1.0,
            },
        };
        let f = Function {
            domain: vec![[0.0, 1.0]],
            range: None,
            kind: Kind::Stitching {
                functions: vec![linear(0.0, 1.0), linear(10.0, 20.0)],
                bounds: vec![0.5],
                encode: vec![[0.0, 1.0], [1.0, 0.0]],
            },
        };
        assert_eq!(eval(&f, &[0.25]), vec![0.5]);
        assert_eq!(eval(&f, &[0.5]), vec![20.0]);
        assert_eq!(eval(&f, &[1.0]), vec![10.0]);
    }

    #[test]
    fn sampled_interpolates_between_its_samples() {
        let f = Function {
            domain: vec![[0.0, 1.0], [0.0, 1.0]],
            range: Some(vec![[0.0, 1.0]]),
            kind: Kind::Sampled {
                size: vec![2, 2],
                encode: vec![[0.0, 1.0], [0.0, 1.0]],
                outputs: 1,
                samples: vec![0.0, 1.0, 0.0, 0.5],
            },
        };
        assert_eq!(eval(&f, &[1.0, 0.0]), vec![1.0]);
        assert_eq!(eval(&f, &[0.5, 1.0]), vec![0.25]);
        assert_eq!(eval(&f, &[1.0, 0.5]), vec![0.75]);
    }

    #[test]
    fn bits_read_across_bytes() {
        let mut bits = Bits {
            data: &[0b1010_1100, 0b0101_0011],
            position: 0,
        };
        assert_eq!(bits.read(3), Some(0b101));
        assert_eq!(bits.read(12), Some(0b0110_0010_1001));
        assert_eq!(bits.read(1), Some(1));
        assert_eq!(bits.read(1), None);
    }
}
