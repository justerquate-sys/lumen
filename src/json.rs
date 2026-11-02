//! A JSON reader for InnerTube responses (0.5–1 MB each): strings without
//! escapes are borrowed from the input and objects are sorted vectors, so a
//! parse allocates a few vectors instead of one `String` per key and string
//! plus a B-tree node per member.
//!
//! It accepts exactly what `serde_json::from_slice::<serde_json::Value>`
//! accepts and reads the same values: UTF-8 input, RFC 8259 grammar, no
//! trailing characters, nesting below 128, no lone surrogates, numbers as
//! u64 / i64 (negative) / f64 (`-0` and fractions/exponents are f64; integers
//! beyond 64 bits become f64; out-of-range floats are an error). Object
//! members iterate in byte order of their keys and a repeated key keeps its
//! last value, like serde_json's default `Map` (a `BTreeMap`). Floats are
//! correctly rounded (Rust's parser; serde_json's default fast path agrees
//! for the short decimals YouTube sends).
//!
//! Outgoing JSON is still built with `serde_json` (`json!`); `to_serde`
//! converts a parsed value for that.

use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value<'a> {
    #[default]
    Null,
    Bool(bool),
    Number(Number),
    String(Cow<'a, str>),
    Array(Vec<Value<'a>>),
    Object(Map<'a>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    U(u64),
    I(i64),
    F(f64),
}

/// Object members sorted by key (byte order), keys unique.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Map<'a>(Vec<(Cow<'a, str>, Value<'a>)>);

pub static NULL: Value<'static> = Value::Null;

/// Test helper: `json!(…)` as a parsed `Value`.
#[cfg(test)]
macro_rules! jv {
    ($($t:tt)+) => {
        $crate::json::Value::from_serde(&serde_json::json!($($t)+))
    };
}
#[cfg(test)]
pub(crate) use jv;

/// Parses a whole document (`None` where serde_json would return an error).
pub fn parse(bytes: &[u8]) -> Option<Value<'_>> {
    let s = std::str::from_utf8(bytes).ok()?;
    let mut p = Parser { s, b: bytes, i: 0, depth: 128 };
    let v = p.value()?;
    p.ws();
    (p.i == p.b.len()).then_some(v)
}

pub fn parse_str(s: &str) -> Option<Value<'_>> {
    parse(s.as_bytes())
}

struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    depth: u32,
}

impl<'a> Parser<'a> {
    #[inline]
    fn ws(&mut self) {
        while let Some(&c) = self.b.get(self.i) {
            if matches!(c, b' ' | b'\n' | b'\r' | b'\t') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn lit(&mut self, word: &[u8], v: Value<'a>) -> Option<Value<'a>> {
        if self.b[self.i..].starts_with(word) {
            self.i += word.len();
            Some(v)
        } else {
            None
        }
    }

    fn value(&mut self) -> Option<Value<'a>> {
        self.ws();
        match *self.b.get(self.i)? {
            b'"' => self.string().map(Value::String),
            b'{' => self.object(),
            b'[' => self.array(),
            b't' => self.lit(b"true", Value::Bool(true)),
            b'f' => self.lit(b"false", Value::Bool(false)),
            b'n' => self.lit(b"null", Value::Null),
            b'-' | b'0'..=b'9' => self.number().map(Value::Number),
            _ => None,
        }
    }

    fn enter(&mut self) -> Option<()> {
        self.depth -= 1;
        (self.depth > 0).then_some(())
    }

    fn array(&mut self) -> Option<Value<'a>> {
        self.enter()?;
        self.i += 1;
        let mut out = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
        } else {
            loop {
                out.push(self.value()?);
                self.ws();
                match self.b.get(self.i)? {
                    b',' => self.i += 1,
                    b']' => {
                        self.i += 1;
                        break;
                    }
                    _ => return None,
                }
            }
        }
        self.depth += 1;
        Some(Value::Array(out))
    }

    fn object(&mut self) -> Option<Value<'a>> {
        self.enter()?;
        self.i += 1;
        let mut out = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
        } else {
            loop {
                self.ws();
                if self.b.get(self.i) != Some(&b'"') {
                    return None;
                }
                let k = self.string()?;
                self.ws();
                if self.b.get(self.i) != Some(&b':') {
                    return None;
                }
                self.i += 1;
                let v = self.value()?;
                out.push((k, v));
                self.ws();
                match self.b.get(self.i)? {
                    b',' => self.i += 1,
                    b'}' => {
                        self.i += 1;
                        break;
                    }
                    _ => return None,
                }
            }
        }
        self.depth += 1;
        Some(Value::Object(Map::from_members(out)))
    }

    /// At the opening quote.
    fn string(&mut self) -> Option<Cow<'a, str>> {
        self.i += 1;
        let start = self.i;
        // Eight bytes at a time while none is a quote, backslash or control byte.
        while let Some(w) = self.b.get(self.i..self.i + 8) {
            if special(u64::from_le_bytes(w.try_into().unwrap())) {
                break;
            }
            self.i += 8;
        }
        loop {
            let c = *self.b.get(self.i)?;
            match c {
                b'"' => {
                    let s = &self.s[start..self.i];
                    self.i += 1;
                    return Some(Cow::Borrowed(s));
                }
                b'\\' => break,
                0..=0x1f => return None,
                _ => self.i += 1,
            }
        }
        // Escapes: copy what we have and continue unescaping.
        let mut out = String::with_capacity(self.i - start + 16);
        out.push_str(&self.s[start..self.i]);
        loop {
            let c = *self.b.get(self.i)?;
            match c {
                b'"' => {
                    self.i += 1;
                    return Some(Cow::Owned(out));
                }
                b'\\' => {
                    self.i += 1;
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let n = self.hex4()?;
                            let ch = match n {
                                0xDC00..=0xDFFF => return None,
                                0xD800..=0xDBFF => {
                                    if self.b.get(self.i) != Some(&b'\\') || self.b.get(self.i + 1) != Some(&b'u') {
                                        return None;
                                    }
                                    self.i += 2;
                                    let n2 = self.hex4()?;
                                    if !(0xDC00..=0xDFFF).contains(&n2) {
                                        return None;
                                    }
                                    char::from_u32((((n - 0xD800) << 10) | (n2 - 0xDC00)) + 0x1_0000)?
                                }
                                n => char::from_u32(n)?,
                            };
                            out.push(ch);
                        }
                        _ => return None,
                    }
                }
                0..=0x1f => return None,
                _ => {
                    // Copy the run up to the next quote, backslash or control byte.
                    let run = self.i;
                    while let Some(&c) = self.b.get(self.i) {
                        if c == b'"' || c == b'\\' || c < 0x20 {
                            break;
                        }
                        self.i += 1;
                    }
                    out.push_str(&self.s[run..self.i]);
                }
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = self.b.get(self.i..self.i + 4)?;
        let mut n = 0u32;
        for &c in h {
            n = n * 16 + (c as char).to_digit(16)?;
        }
        self.i += 4;
        Some(n)
    }

    fn digits(&mut self) -> usize {
        let s = self.i;
        while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
        }
        self.i - s
    }

    fn number(&mut self) -> Option<Number> {
        let start = self.i;
        let neg = self.b[self.i] == b'-';
        if neg {
            self.i += 1;
        }
        let int_start = self.i;
        match *self.b.get(self.i)? {
            b'0' => {
                self.i += 1;
                if self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
            }
            b'1'..=b'9' => {
                self.digits();
            }
            _ => return None,
        }
        let int_end = self.i;
        let mut float = false;
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if self.digits() == 0 {
                return None;
            }
            float = true;
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                return None;
            }
            float = true;
        }
        let _ = (int_start, int_end, float);
        number_value(&self.b[start..self.i])
    }
}

/// Whether any byte of `w` is `"`, `\\` or below 0x20 (may also flag a
/// byte after a real hit, which the bytewise loop then handles).
#[inline]
fn special(w: u64) -> bool {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let q = w ^ (LO * b'"' as u64);
    let bs = w ^ (LO * b'\\' as u64);
    ((q.wrapping_sub(LO) & !q) | (bs.wrapping_sub(LO) & !bs) | (w.wrapping_sub(LO * 0x20) & !w)) & HI != 0
}

/// The value serde_json reads for a number token (grammar already checked):
/// its non-`float_roundtrip` algorithm, so floats match it bit for bit.
fn number_value(t: &[u8]) -> Option<Number> {
    let positive = t[0] != b'-';
    let mut i = usize::from(!positive);
    let mut sig = 0u64;
    if t[i] == b'0' {
        i += 1;
    } else {
        while let Some(&c) = t.get(i).filter(|c| c.is_ascii_digit()) {
            let d = u64::from(c - b'0');
            if overflows(sig, d, u64::MAX) {
                // Too long for u64: further integer digits only scale.
                let mut exp = 0i32;
                while t.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                    exp += 1;
                }
                return float_tail(positive, sig, exp, t, i).map(Number::F);
            }
            sig = sig * 10 + d;
            i += 1;
        }
    }
    if i == t.len() {
        if positive {
            return Some(Number::U(sig));
        }
        let n = (sig as i64).wrapping_neg();
        // `-0` and values below i64::MIN become f64.
        return Some(if n >= 0 { Number::F(-(sig as f64)) } else { Number::I(n) });
    }
    float_tail(positive, sig, 0, t, i).map(Number::F)
}

fn overflows(a: u64, b: u64, max: u64) -> bool {
    a >= max / 10 && (a > max / 10 || b > max % 10)
}

fn float_tail(positive: bool, mut sig: u64, mut exp: i32, t: &[u8], mut i: usize) -> Option<f64> {
    if t.get(i) == Some(&b'.') {
        i += 1;
        while let Some(&c) = t.get(i).filter(|c| c.is_ascii_digit()) {
            let d = u64::from(c - b'0');
            if overflows(sig, d, u64::MAX) {
                while t.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                break;
            }
            sig = sig * 10 + d;
            exp -= 1;
            i += 1;
        }
    }
    if matches!(t.get(i), Some(b'e' | b'E')) {
        i += 1;
        let positive_exp = match t.get(i) {
            Some(b'+') => {
                i += 1;
                true
            }
            Some(b'-') => {
                i += 1;
                false
            }
            _ => true,
        };
        let mut e = 0i32;
        while let Some(&c) = t.get(i).filter(|c| c.is_ascii_digit()) {
            let d = i32::from(c - b'0');
            if e >= i32::MAX / 10 && (e > i32::MAX / 10 || d > i32::MAX % 10) {
                if sig != 0 && positive_exp {
                    return None;
                }
                return Some(if positive { 0.0 } else { -0.0 });
            }
            e = e * 10 + d;
            i += 1;
        }
        exp = if positive_exp { exp.saturating_add(e) } else { exp.saturating_sub(e) };
    }
    let mut f = sig as f64;
    loop {
        match POW10.get(exp.wrapping_abs() as usize) {
            Some(&pow) => {
                if exp >= 0 {
                    f *= pow;
                    if f.is_infinite() {
                        return None;
                    }
                } else {
                    f /= pow;
                }
                break;
            }
            None => {
                if f == 0.0 {
                    break;
                }
                if exp >= 0 {
                    return None;
                }
                f /= 1e308;
                exp += 308;
            }
        }
    }
    Some(if positive { f } else { -f })
}

#[rustfmt::skip]
static POW10: [f64; 309] = [
    1e000, 1e001, 1e002, 1e003, 1e004, 1e005, 1e006, 1e007, 1e008, 1e009,
    1e010, 1e011, 1e012, 1e013, 1e014, 1e015, 1e016, 1e017, 1e018, 1e019,
    1e020, 1e021, 1e022, 1e023, 1e024, 1e025, 1e026, 1e027, 1e028, 1e029,
    1e030, 1e031, 1e032, 1e033, 1e034, 1e035, 1e036, 1e037, 1e038, 1e039,
    1e040, 1e041, 1e042, 1e043, 1e044, 1e045, 1e046, 1e047, 1e048, 1e049,
    1e050, 1e051, 1e052, 1e053, 1e054, 1e055, 1e056, 1e057, 1e058, 1e059,
    1e060, 1e061, 1e062, 1e063, 1e064, 1e065, 1e066, 1e067, 1e068, 1e069,
    1e070, 1e071, 1e072, 1e073, 1e074, 1e075, 1e076, 1e077, 1e078, 1e079,
    1e080, 1e081, 1e082, 1e083, 1e084, 1e085, 1e086, 1e087, 1e088, 1e089,
    1e090, 1e091, 1e092, 1e093, 1e094, 1e095, 1e096, 1e097, 1e098, 1e099,
    1e100, 1e101, 1e102, 1e103, 1e104, 1e105, 1e106, 1e107, 1e108, 1e109,
    1e110, 1e111, 1e112, 1e113, 1e114, 1e115, 1e116, 1e117, 1e118, 1e119,
    1e120, 1e121, 1e122, 1e123, 1e124, 1e125, 1e126, 1e127, 1e128, 1e129,
    1e130, 1e131, 1e132, 1e133, 1e134, 1e135, 1e136, 1e137, 1e138, 1e139,
    1e140, 1e141, 1e142, 1e143, 1e144, 1e145, 1e146, 1e147, 1e148, 1e149,
    1e150, 1e151, 1e152, 1e153, 1e154, 1e155, 1e156, 1e157, 1e158, 1e159,
    1e160, 1e161, 1e162, 1e163, 1e164, 1e165, 1e166, 1e167, 1e168, 1e169,
    1e170, 1e171, 1e172, 1e173, 1e174, 1e175, 1e176, 1e177, 1e178, 1e179,
    1e180, 1e181, 1e182, 1e183, 1e184, 1e185, 1e186, 1e187, 1e188, 1e189,
    1e190, 1e191, 1e192, 1e193, 1e194, 1e195, 1e196, 1e197, 1e198, 1e199,
    1e200, 1e201, 1e202, 1e203, 1e204, 1e205, 1e206, 1e207, 1e208, 1e209,
    1e210, 1e211, 1e212, 1e213, 1e214, 1e215, 1e216, 1e217, 1e218, 1e219,
    1e220, 1e221, 1e222, 1e223, 1e224, 1e225, 1e226, 1e227, 1e228, 1e229,
    1e230, 1e231, 1e232, 1e233, 1e234, 1e235, 1e236, 1e237, 1e238, 1e239,
    1e240, 1e241, 1e242, 1e243, 1e244, 1e245, 1e246, 1e247, 1e248, 1e249,
    1e250, 1e251, 1e252, 1e253, 1e254, 1e255, 1e256, 1e257, 1e258, 1e259,
    1e260, 1e261, 1e262, 1e263, 1e264, 1e265, 1e266, 1e267, 1e268, 1e269,
    1e270, 1e271, 1e272, 1e273, 1e274, 1e275, 1e276, 1e277, 1e278, 1e279,
    1e280, 1e281, 1e282, 1e283, 1e284, 1e285, 1e286, 1e287, 1e288, 1e289,
    1e290, 1e291, 1e292, 1e293, 1e294, 1e295, 1e296, 1e297, 1e298, 1e299,
    1e300, 1e301, 1e302, 1e303, 1e304, 1e305, 1e306, 1e307, 1e308,
];

impl<'a> Map<'a> {
    /// Sorts by key and keeps the last value of a repeated key.
    fn from_members(mut m: Vec<(Cow<'a, str>, Value<'a>)>) -> Self {
        if m.len() > 1 && !m.windows(2).all(|w| w[0].0 < w[1].0) {
            m.sort_by(|a, b| a.0.cmp(&b.0)); // stable: equal keys stay in document order
            let mut out: Vec<(Cow<'a, str>, Value<'a>)> = Vec::with_capacity(m.len());
            for (k, v) in m {
                match out.last_mut() {
                    Some(last) if last.0 == k => last.1 = v,
                    _ => out.push((k, v)),
                }
            }
            m = out;
        }
        Map(m)
    }
    pub fn get(&self, k: &str) -> Option<&Value<'a>> {
        if self.0.len() <= 8 {
            self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v)
        } else {
            self.0.binary_search_by(|(key, _)| key.as_ref().cmp(k)).ok().map(|i| &self.0[i].1)
        }
    }
    pub fn contains_key(&self, k: &str) -> bool {
        self.get(k).is_some()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value<'a>)> {
        self.0.iter().map(|(k, v)| (k.as_ref(), v))
    }
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_ref())
    }
    pub fn values(&self) -> impl Iterator<Item = &Value<'a>> {
        self.0.iter().map(|(_, v)| v)
    }
}

impl<'m, 'a> IntoIterator for &'m Map<'a> {
    type Item = (&'m str, &'m Value<'a>);
    type IntoIter = std::iter::Map<std::slice::Iter<'m, (Cow<'a, str>, Value<'a>)>, fn(&'m (Cow<'a, str>, Value<'a>)) -> (&'m str, &'m Value<'a>)>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|(k, v)| (k.as_ref(), v))
    }
}

/// `get` by key (objects) or position (arrays), like `serde_json::Value::get`.
pub trait Index {
    fn index_into<'v, 'a>(&self, v: &'v Value<'a>) -> Option<&'v Value<'a>>;
}

impl Index for str {
    fn index_into<'v, 'a>(&self, v: &'v Value<'a>) -> Option<&'v Value<'a>> {
        match v {
            Value::Object(m) => m.get(self),
            _ => None,
        }
    }
}

impl Index for String {
    fn index_into<'v, 'a>(&self, v: &'v Value<'a>) -> Option<&'v Value<'a>> {
        self.as_str().index_into(v)
    }
}

impl Index for usize {
    fn index_into<'v, 'a>(&self, v: &'v Value<'a>) -> Option<&'v Value<'a>> {
        match v {
            Value::Array(a) => a.get(*self),
            _ => None,
        }
    }
}

impl<T: Index + ?Sized> Index for &T {
    fn index_into<'v, 'a>(&self, v: &'v Value<'a>) -> Option<&'v Value<'a>> {
        (**self).index_into(v)
    }
}

impl<'a> Value<'a> {
    pub fn get<I: Index>(&self, i: I) -> Option<&Value<'a>> {
        i.index_into(self)
    }

    /// RFC 6901 pointer, with serde_json's array index rules.
    pub fn pointer(&self, p: &str) -> Option<&Value<'a>> {
        if p.is_empty() {
            return Some(self);
        }
        if !p.starts_with('/') {
            return None;
        }
        let mut cur = self;
        for tok in p.split('/').skip(1) {
            let owned;
            let t: &str = if tok.contains('~') {
                owned = tok.replace("~1", "/").replace("~0", "~");
                &owned
            } else {
                tok
            };
            cur = match cur {
                Value::Object(m) => m.get(t)?,
                Value::Array(a) => {
                    if t.starts_with('+') || (t.starts_with('0') && t.len() != 1) {
                        return None;
                    }
                    a.get(t.parse::<usize>().ok()?)?
                }
                _ => return None,
            };
        }
        Some(cur)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Value<'a>>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_object(&self) -> Option<&Map<'a>> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(Number::U(n)) => Some(*n),
            Value::Number(Number::I(n)) => u64::try_from(*n).ok(),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Number(Number::U(n)) => i64::try_from(*n).ok(),
            Value::Number(Number::I(n)) => Some(*n),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(Number::U(n)) => Some(*n as f64),
            Value::Number(Number::I(n)) => Some(*n as f64),
            Value::Number(Number::F(f)) => Some(*f),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }
    pub fn is_array(&self) -> bool {
        matches!(self, Value::Array(_))
    }
    pub fn is_string(&self) -> bool {
        matches!(self, Value::String(_))
    }

    /// A copy that owns all its strings.
    pub fn into_owned(self) -> Value<'static> {
        match self {
            Value::Null => Value::Null,
            Value::Bool(b) => Value::Bool(b),
            Value::Number(n) => Value::Number(n),
            Value::String(s) => Value::String(Cow::Owned(s.into_owned())),
            Value::Array(a) => Value::Array(a.into_iter().map(Value::into_owned).collect()),
            Value::Object(m) => Value::Object(Map(m.0.into_iter().map(|(k, v)| (Cow::Owned(k.into_owned()), v.into_owned())).collect())),
        }
    }

    /// The same value from a `serde_json::Value` (values built with `json!`).
    pub fn from_serde(v: &serde_json::Value) -> Value<'static> {
        use serde_json::Value as J;
        match v {
            J::Null => Value::Null,
            J::Bool(b) => Value::Bool(*b),
            J::Number(n) => Value::Number(if let Some(u) = n.as_u64() {
                Number::U(u)
            } else if let Some(i) = n.as_i64() {
                Number::I(i)
            } else {
                Number::F(n.as_f64().unwrap_or(0.0))
            }),
            J::String(s) => Value::String(Cow::Owned(s.clone())),
            J::Array(a) => Value::Array(a.iter().map(Value::from_serde).collect()),
            J::Object(m) => Value::Object(Map(m.iter().map(|(k, v)| (Cow::Owned(k.clone()), Value::from_serde(v))).collect())),
        }
    }

    /// The same value as `serde_json::Value` (to embed in outgoing JSON).
    pub fn to_serde(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match self {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Number(Number::U(n)) => J::from(*n),
            Value::Number(Number::I(n)) => J::from(*n),
            Value::Number(Number::F(f)) => serde_json::Number::from_f64(*f).map_or(J::Null, J::Number),
            Value::String(s) => J::String(s.to_string()),
            Value::Array(a) => J::Array(a.iter().map(Value::to_serde).collect()),
            Value::Object(m) => J::Object(m.iter().map(|(k, v)| (k.to_string(), v.to_serde())).collect()),
        }
    }
}

impl<'a, I: Index> std::ops::Index<I> for Value<'a> {
    type Output = Value<'a>;
    fn index(&self, i: I) -> &Value<'a> {
        // `NULL` is 'static and `Value` is covariant in its lifetime.
        i.index_into(self).unwrap_or(&NULL)
    }
}

impl std::fmt::Display for Value<'_> {
    /// Compact JSON, byte for byte what serde_json writes for the same value.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_serde().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Our reading vs serde_json's for one document (both accept or both reject; same value).
    fn same(doc: &str) {
        let ours = parse(doc.as_bytes());
        let theirs = serde_json::from_str::<serde_json::Value>(doc).ok();
        match (&ours, &theirs) {
            (Some(o), Some(t)) => assert_eq!(&o.to_serde(), t, "{doc}"),
            (None, None) => {}
            _ => panic!("acceptance differs for {doc:?}: ours {ours:?}, serde_json {theirs:?}"),
        }
    }

    #[test]
    fn agrees_with_serde_json() {
        for d in [
            "null", "true", "false", " 1 ", "0", "-0", "-1", "01", "1.", ".5", "1e", "1e+", "1E-2", "-", "--1", "+1",
            "18446744073709551615", "18446744073709551616", "-9223372036854775808", "-9223372036854775809",
            "1.5e308", "1e309", "-1e309", "0.1", "3.14159", "2.5E+3", "123456789012345678901234567890",
            r#""a""#, r#""é\n\t\"\\\/\b\f\r""#, r#""😀""#, r#""\ud83d""#, r#""\ude00""#, r#""\ud83dx""#,
            r#""\ud83dA""#, r#""\x""#, r#""\u12""#, "\"a\u{1}b\"", "\"é😀\"", r#""tab	in""#,
            "[]", "[1,]", "[,1]", "[1 2]", "{}", r#"{"a":1,}"#, r#"{"a" 1}"#, r#"{1:2}"#, r#"{"b":1,"a":2,"b":3}"#,
            r#"{"z":{"y":[1,{"x":null}]},"a":"A"}"#, "[1] x", "", " ", "nul", "truex", "[true,false,null]",
            r#"{"k":"v"}  "#, "\u{feff}1", "0.18652007650833817", "1.7976931348623157e308", "4.9e-324", "1e-400", "-1e-400",
            "0e99999999999", "1e99999999999", "1e-99999999999", "-0.0", "12345678901234567890.5", "123456789012345678901e-5",
            "0.000000000000000000000000000001234567890123456789", "1E400", "0E400", "9007199254740993", "-9007199254740993.0",
        ] {
            same(d);
        }
        same(&"[".repeat(127).to_string().chars().chain("]".repeat(127).chars()).collect::<String>());
        same(&"[".repeat(128).to_string().chars().chain("]".repeat(128).chars()).collect::<String>());
        assert!(parse(b"\"\xff\"").is_none() && serde_json::from_slice::<serde_json::Value>(b"\"\xff\"").is_err());
    }

    #[test]
    fn fixtures_read_the_same() {
        for f in ["search.json", "next.json", "browse_cont.json", "channel.json", "comments.json", "player.json", "chapters_next.json"] {
            let bytes = crate::util::fixture_bytes(f);
            let ours = parse(&bytes).expect(f);
            let theirs: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(ours.to_serde() == theirs, "{f}");
            assert_eq!(ours.to_string(), theirs.to_string(), "{f}");
        }
    }

    /// Truncated and corrupted copies of a real response: both readers agree on every one.
    #[test]
    fn mangled_documents_agree() {
        let bytes = crate::util::fixture_bytes("player.json");
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let alphabet = b"{}[]\",:0123456789.eE+-tfnu \\\x01\xc3\xa9\xff";
        for _ in 0..3000 {
            let mut d = bytes.clone();
            for _ in 0..1 + rnd(3) {
                let at = rnd(d.len());
                match rnd(3) {
                    0 => d[at] = alphabet[rnd(alphabet.len())],
                    1 => {
                        d.remove(at);
                    }
                    _ => d.insert(at, alphabet[rnd(alphabet.len())]),
                }
            }
            if rnd(4) == 0 {
                d.truncate(rnd(d.len()));
            }
            let ours = parse(&d).map(|v| v.to_serde());
            let theirs = serde_json::from_slice::<serde_json::Value>(&d).ok();
            assert_eq!(ours, theirs, "{}", String::from_utf8_lossy(&d));
        }
    }

    #[test]
    fn map_order_and_duplicates() {
        let v = parse(br#"{"b":1,"a":2,"b":3,"c":{"x":1}}"#).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["a", "b", "c"]);
        assert_eq!(v.get("b").and_then(Value::as_u64), Some(3));
        assert_eq!(v["c"]["x"].as_u64(), Some(1));
        assert!(v["nope"]["x"].is_null());
        assert_eq!(v.pointer("/c/x").and_then(Value::as_u64), Some(1));
        let a = parse(br#"{"a":[10,20],"a/b":{"~":5}}"#).unwrap();
        assert_eq!(a.pointer("/a/1").and_then(Value::as_u64), Some(20));
        assert_eq!(a.pointer("/a/01"), None);
        assert_eq!(a.pointer("/a~1b/~0").and_then(Value::as_u64), Some(5));
        assert_eq!(a.pointer(""), Some(&a));
        assert_eq!(a.pointer("a"), None);
        assert!(matches!(parse(br#""plain""#), Some(Value::String(Cow::Borrowed("plain")))));
    }
}
