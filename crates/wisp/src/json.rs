//! JSON in: a strict parser for request bodies, and [`FromJson`], which
//! reads a parsed value into a Rust type and keeps every problem it finds,
//! by field, so an API can answer all of them at once (a 422).
//!
//! Written here rather than pulled in, like the rest of the runtime: JSON is
//! a small, fixed grammar (RFC 8259). The parser accepts exactly that, with
//! no comments, trailing commas or bare words, and nests at most `DEPTH`
//! deep so a hostile body cannot overflow the stack.

use crate::{Error, Json};
use std::collections::{BTreeMap, HashMap};
use std::hash::BuildHasher;

/// A parsed JSON value. Numbers keep their text, so a `u64` or an `i128`
/// reads back exactly. An object keeps its members in order; when a key
/// appears twice the last one counts, as in JavaScript.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// A member of an object; `None` for anything else.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Number(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// What it is, for a message: "a string", "null".
    fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Number(_) => "a number",
            Value::String(_) => "a string",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        }
    }
}

impl Json for Value {
    fn json(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => b.json(out),
            Value::Number(n) => out.push_str(n),
            Value::String(s) => s.json(out),
            Value::Array(items) => items.json(out),
            Value::Object(members) => {
                out.push('{');
                for (k, (key, v)) in members.iter().enumerate() {
                    if k > 0 {
                        out.push(',');
                    }
                    key.json(out);
                    out.push(':');
                    v.json(out);
                }
                out.push('}');
            }
        }
    }
}

/// How deep arrays and objects may nest.
const DEPTH: u32 = 128;

/// Parses `text`, which must be one JSON value (whitespace around it is
/// fine). The error says what was wrong and where: `expected `:` at line 1,
/// column 9`.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser {
        s: text,
        b: text.as_bytes(),
        i: 0,
        depth: 0,
    };
    let value = p.value().and_then(|v| {
        p.space();
        if p.i < p.b.len() {
            return Err("unexpected text after the value");
        }
        Ok(v)
    });
    value.map_err(|what| {
        let before = &text.as_bytes()[..p.i.min(text.len())];
        let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
        let column = before.iter().rev().take_while(|&&b| b != b'\n').count() + 1;
        format!("{what} at line {line}, column {column}")
    })
}

struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    depth: u32,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.space();
        let found = self.peek() == Some(byte);
        self.i += found as usize;
        found
    }

    fn word(&mut self, word: &str, value: Value) -> Result<Value, &'static str> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(value)
        } else {
            Err("expected a value")
        }
    }

    fn value(&mut self) -> Result<Value, &'static str> {
        self.space();
        match self.peek() {
            None => Err("expected a value, found the end"),
            Some(b'n') => self.word("null", Value::Null),
            Some(b't') => self.word("true", Value::Bool(true)),
            Some(b'f') => self.word("false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b'[') => {
                self.nest()?;
                let mut items = Vec::new();
                if !self.eat(b']') {
                    loop {
                        items.push(self.value()?);
                        if self.eat(b']') {
                            break;
                        }
                        if !self.eat(b',') {
                            return Err("expected `,` or `]`");
                        }
                    }
                }
                self.depth -= 1;
                Ok(Value::Array(items))
            }
            Some(b'{') => {
                self.nest()?;
                let mut members = Vec::new();
                if !self.eat(b'}') {
                    loop {
                        self.space();
                        if self.peek() != Some(b'"') {
                            return Err("expected a key in quotes");
                        }
                        let key = self.string()?;
                        if !self.eat(b':') {
                            return Err("expected `:`");
                        }
                        members.push((key, self.value()?));
                        if self.eat(b'}') {
                            break;
                        }
                        if !self.eat(b',') {
                            return Err("expected `,` or `}`");
                        }
                    }
                }
                self.depth -= 1;
                Ok(Value::Object(members))
            }
            Some(_) => Err("expected a value"),
        }
    }

    fn nest(&mut self) -> Result<(), &'static str> {
        self.depth += 1;
        if self.depth > DEPTH {
            return Err("nested too deeply");
        }
        self.i += 1;
        Ok(())
    }

    fn number(&mut self) -> Result<Value, &'static str> {
        let start = self.i;
        let digits = |p: &mut Parser| {
            let from = p.i;
            while p.peek().is_some_and(|b| b.is_ascii_digit()) {
                p.i += 1;
            }
            p.i > from
        };
        self.i += (self.peek() == Some(b'-')) as usize;
        // No leading zeros: `0`, or a digit from 1 to 9 and more digits.
        if self.peek() == Some(b'0') {
            self.i += 1;
        } else if !digits(self) {
            return Err("expected a digit");
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if !digits(self) {
                return Err("expected a digit after `.`");
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !digits(self) {
                return Err("expected a digit in the exponent");
            }
        }
        if self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'.')
        {
            return Err("expected a number");
        }
        // Only ASCII was taken, so these are character boundaries.
        Ok(Value::Number(self.s[start..self.i].to_string()))
    }

    /// A string, at its opening quote. Runs of plain text are copied whole:
    /// they start and end at ASCII bytes, so at character boundaries of the
    /// text, which is UTF-8 already.
    fn string(&mut self) -> Result<String, &'static str> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let run = self.i;
            while self
                .peek()
                .is_some_and(|b| b != b'"' && b != b'\\' && b >= 0x20)
            {
                self.i += 1;
            }
            out.push_str(&self.s[run..self.i]);
            let Some(b) = self.peek() else {
                return Err("unterminated string");
            };
            self.i += 1;
            match b {
                b'"' => return Ok(out),
                0..0x20 => {
                    self.i -= 1;
                    return Err("control character in a string");
                }
                _ => {
                    let Some(e) = self.peek() else {
                        return Err("unterminated string");
                    };
                    self.i += 1;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let high = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&high) {
                                if !self.b[self.i..].starts_with(b"\\u") {
                                    return Err("unpaired surrogate");
                                }
                                self.i += 2;
                                let low = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&low) {
                                    return Err("unpaired surrogate");
                                }
                                0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00)
                            } else {
                                high
                            };
                            char::from_u32(code).ok_or("unpaired surrogate")?
                        }
                        _ => {
                            self.i -= 1;
                            return Err("unknown escape");
                        }
                    };
                    out.push(c);
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, &'static str> {
        let digits = self
            .b
            .get(self.i..self.i + 4)
            .ok_or("expected 4 hex digits")?;
        let n = digits
            .iter()
            .try_fold(0, |n, &d| Some(n << 4 | (d as char).to_digit(16)?))
            .ok_or("expected 4 hex digits")?;
        self.i += 4;
        Ok(n)
    }
}

/// JSON text read straight into a type ([`FromJson::read`]), with no
/// [`Value`] between: the usual body, its strings without escapes, read
/// once. What it does not read so (an escape, a key twice, a value that is
/// not its type's, a check that fails) it leaves: `None`, and the body is
/// read through a `Value` as ever, which says what is wrong. So what it
/// reads is what that reads, by the same grammar.
#[doc(hidden)]
pub struct Direct<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
    depth: u32,
}

impl<'a> Direct<'a> {
    /// `text` as one `T`, all of it.
    fn whole<T: FromJson>(text: &'a str) -> Option<T> {
        let mut d = Direct {
            s: text,
            b: text.as_bytes(),
            i: 0,
            depth: 0,
        };
        let v = T::read(&mut d)?;
        d.space();
        (d.i == d.b.len()).then_some(v)
    }

    fn space(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    /// Past `byte`, after any space, if it is next.
    fn eat(&mut self, byte: u8) -> bool {
        self.space();
        let found = self.b.get(self.i) == Some(&byte);
        self.i += found as usize;
        found
    }

    /// Past `word` (`null`, `true`, `false`), if it is next.
    fn word(&mut self, word: &[u8]) -> bool {
        self.space();
        let found = self.b[self.i..].starts_with(word);
        self.i += found as usize * word.len();
        found
    }

    pub fn null(&mut self) -> bool {
        self.word(b"null")
    }

    pub fn boolean(&mut self) -> Option<bool> {
        if self.word(b"true") {
            Some(true)
        } else {
            self.word(b"false").then_some(false)
        }
    }

    /// A string with no escape, as it is in the text.
    pub fn string(&mut self) -> Option<&'a str> {
        if !self.eat(b'"') {
            return None;
        }
        use crate::swar;
        let start = self.i;
        // To its quote, an escape or a control byte, eight bytes at a time.
        loop {
            if self.i + 8 > self.b.len() {
                let plain = |b: &u8| *b != b'"' && *b != b'\\' && *b >= 0x20;
                while self.b.get(self.i).is_some_and(plain) {
                    self.i += 1;
                }
                break;
            }
            let x = swar::word(self.b, self.i);
            let stop = swar::eq(x, b'"') | swar::eq(x, b'\\') | swar::below(x, 0x20);
            if stop != 0 {
                self.i += swar::first(stop);
                break;
            }
            self.i += 8;
        }
        // Only ASCII ends it, so a character boundary.
        (self.b.get(self.i) == Some(&b'"')).then(|| {
            self.i += 1;
            &self.s[start..self.i - 1]
        })
    }

    /// A number's text, by `Parser::number`'s grammar.
    pub fn number(&mut self) -> Option<&'a str> {
        self.space();
        let start = self.i;
        let digits = |d: &mut Direct| {
            let from = d.i;
            while d.b.get(d.i).is_some_and(u8::is_ascii_digit) {
                d.i += 1;
            }
            d.i > from
        };
        self.i += (self.b.get(self.i) == Some(&b'-')) as usize;
        if self.b.get(self.i) == Some(&b'0') {
            self.i += 1;
        } else if !digits(self) {
            return None;
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if !digits(self) {
                return None;
            }
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !digits(self) {
                return None;
            }
        }
        if (self.b.get(self.i)).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'.') {
            return None;
        }
        Some(&self.s[start..self.i])
    }

    /// Into an array or object, at its `open`: no deeper than `DEPTH`.
    fn open(&mut self, open: u8) -> Option<()> {
        self.depth += 1;
        (self.depth <= DEPTH && self.eat(open)).then_some(())
    }

    /// Past `close` or `,` after the first, in an array or object: whether
    /// there is another item (`first`: none yet).
    fn more(&mut self, first: &mut bool, close: u8) -> Option<bool> {
        if self.eat(close) {
            self.depth -= 1;
            return Some(false);
        }
        if !std::mem::take(first) && !self.eat(b',') {
            return None;
        }
        Some(true)
    }

    pub fn array(&mut self) -> Option<()> {
        self.open(b'[')
    }

    /// Whether the array has another item, which comes next.
    pub fn item(&mut self, first: &mut bool) -> Option<bool> {
        self.more(first, b']')
    }

    pub fn object(&mut self) -> Option<()> {
        self.open(b'{')
    }

    /// The object's next key, its value next; `Some(None)` at its end.
    pub fn key(&mut self, first: &mut bool) -> Option<Option<&'a str>> {
        if !self.more(first, b'}')? {
            return Some(None);
        }
        let key = self.string()?;
        self.eat(b':').then_some(Some(key))
    }

    /// Past a value of any kind, which no field takes.
    pub fn skip(&mut self) -> Option<()> {
        self.space();
        let mut first = true;
        match self.b.get(self.i)? {
            b'"' => self.string().map(drop),
            b'-' | b'0'..=b'9' => self.number().map(drop),
            b'[' => {
                self.array()?;
                while self.item(&mut first)? {
                    self.skip()?;
                }
                Some(())
            }
            b'{' => {
                self.object()?;
                while self.key(&mut first)?.is_some() {
                    self.skip()?;
                }
                Some(())
            }
            _ => (self.null() || self.boolean().is_some()).then_some(()),
        }
    }
}

/// What went wrong while reading a value, by where: `title`,
/// `author.name`, `tags[2]` (`body` for the value itself).
#[derive(Debug, Default)]
pub struct Problems {
    at: String,
    list: Vec<(String, String)>,
    /// The text of `list`.
    bytes: usize,
    /// Reading a form, whose values are all text: a number or `bool` may
    /// be text, a blank field is missing, and one value is a list of one.
    form: bool,
}

/// The most problems one answer lists, and about the most bytes of them.
const MOST: usize = 100;
const MOST_BYTES: usize = 16 * 1024;

impl Problems {
    /// For reading an object built from a form (see `input::whole`).
    pub(crate) fn form() -> Problems {
        Problems {
            form: true,
            ..Problems::default()
        }
    }

    /// Every problem, by field, as one 422.
    pub(crate) fn into_error(self) -> Error {
        Error::invalid_fields(self.list)
    }

    /// Notes `message` about the value being read. Past `MOST` problems, or
    /// `MOST_BYTES` of them, the rest are not noted: a body of a million
    /// bad items is answered with the first, not a million messages.
    pub fn add(&mut self, message: impl Into<String>) {
        if self.list.len() >= MOST || self.bytes > MOST_BYTES {
            return;
        }
        let at = if self.at.is_empty() { "body" } else { &self.at };
        // One message per place: the first says enough.
        if !self.list.iter().any(|(a, _)| a == at) {
            let message = message.into();
            self.bytes += at.len() + message.len();
            self.list.push((at.to_string(), message));
        }
    }

    /// Reads `v` as a `T`, noting what is wrong with it under `segment` (a
    /// field's name, or `[i]`).
    pub fn read<T: FromJson>(&mut self, segment: &str, v: &Value) -> Option<T> {
        let len = self.at.len();
        if !self.at.is_empty() && !segment.starts_with('[') {
            self.at.push('.');
        }
        self.at.push_str(segment);
        let out = T::from_json(v, self);
        self.at.truncate(len);
        out
    }

    /// [`Problems::read`] for item `i` of an array, without a `String` for
    /// its `[i]`.
    fn item<T: FromJson>(&mut self, i: usize, v: &Value) -> Option<T> {
        use std::fmt::Write;
        let len = self.at.len();
        let _ = write!(self.at, "[{i}]");
        let out = T::from_json(v, self);
        self.at.truncate(len);
        out
    }

    /// The members of an object, or a note that `v` is not one.
    pub fn object<'v>(&mut self, v: &'v Value) -> Option<&'v [(String, Value)]> {
        match v {
            Value::Object(members) => Some(members),
            other => {
                self.add(format!("expected an object, found {}", other.kind()));
                None
            }
        }
    }

    /// [`Problems::field`] for a field with `#[json(was = "old")]` (`was`,
    /// `""` for none), which old rows still name that way, and
    /// `#[json(default)]`: `absent` is what it is when it is not there.
    pub fn field_or<T: FromJson>(
        &mut self,
        members: &[(String, Value)],
        name: &str,
        was: &str,
        absent: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let blank = |v: &Value| matches!(v, Value::String(s) if s.trim().is_empty());
        let has = |n: &str| {
            members
                .iter()
                .any(|(k, v)| k == n && !(self.form && blank(v)))
        };
        let name = if !was.is_empty() && !has(name) && has(was) {
            was
        } else {
            name
        };
        if !has(name)
            && let Some(v) = absent()
        {
            return Some(v);
        }
        self.field(members, name)
    }

    /// The field `name` of an object: absent is `T::missing()`, which is a
    /// problem for anything but an `Option`.
    pub fn field<T: FromJson>(&mut self, members: &[(String, Value)], name: &str) -> Option<T> {
        let blank = |v: &Value| matches!(v, Value::String(s) if s.trim().is_empty());
        // A form's blank field is one left out: required, or `None`.
        let found = members
            .iter()
            .rev()
            .find(|(k, v)| k == name && !(self.form && blank(v)));
        match found {
            Some((_, v)) => self.read(name, v),
            None => T::missing().or_else(|| {
                self.read::<Missing>(name, &Value::Null);
                None
            }),
        }
    }

    /// `message` about the field `name`, when there is one: a validation
    /// that did not pass.
    pub fn check(&mut self, name: &str, message: Option<String>) {
        if let Some(m) = message {
            let len = self.at.len();
            if !self.at.is_empty() {
                self.at.push('.');
            }
            self.at.push_str(name);
            self.add(m);
            self.at.truncate(len);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// Stands for an absent field, to note it at the field's place.
struct Missing;

impl FromJson for Missing {
    fn from_json(_: &Value, p: &mut Problems) -> Option<Missing> {
        p.add("is required");
        None
    }
}

/// A type an API reads from JSON: a request body as `body: T`, or any value
/// with [`from_json`]. `#[derive(FromJson)]` implements it for a struct
/// (from an object with its fields; `Option` ones may be left out) and a
/// fieldless enum (from its variant's name), with checks such as
/// `#[validate(min_len = 1)]` on fields.
///
/// Implemented for strings, numbers, `bool`, `Option`, `Vec`, `Box`, maps
/// with string keys and [`Value`] (anything).
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be read from JSON: it does not implement `wisp::FromJson`",
    note = "add `#[derive(FromJson)]` to the type (it is in `wisp::prelude`)"
)]
pub trait FromJson: Sized {
    /// Reads `v`, or notes in `problems` why not and returns `None`.
    fn from_json(v: &Value, problems: &mut Problems) -> Option<Self>;

    /// The value of a field that was left out, if it may be.
    fn missing() -> Option<Self> {
        None
    }

    /// Reads itself straight from JSON text (see [`Direct`]); `None`
    /// leaves it to [`FromJson::from_json`], through a [`Value`].
    #[doc(hidden)]
    fn read(d: &mut Direct) -> Option<Self> {
        let _ = d;
        None
    }

    /// The field marked `#[unique]`: its name and how to read it, which a
    /// saved [`Table`](crate::Table) of the type refuses a repeat of.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)]
    const UNIQUE: Option<(&'static str, fn(&Self) -> &str)> = None;

    /// Whether [`FromJson::seal`] does anything: a `Password` is in it.
    #[doc(hidden)]
    const SEALS: bool = false;

    /// Run by a table on a row as it enters (`hash`), or as it is read from
    /// the store: a `Password` read as text becomes its hash.
    #[doc(hidden)]
    fn seal(&mut self, hash: bool) {
        let _ = hash;
    }
}

/// Reads a JSON body into a `T`. Text that is not JSON is a 400 that says
/// where; JSON that is not a `T`, or does not pass its checks, is a 422
/// that lists every problem by field ([`Error::fields`]).
pub fn from_json<T: FromJson>(body: &[u8]) -> crate::Result<T> {
    let text = std::str::from_utf8(body).map_err(|_| Error::new(400, "The body is not UTF-8"))?;
    if text.trim().is_empty() {
        return T::missing().ok_or_else(|| Error::new(400, "Expected a JSON body"));
    }
    if let Some(v) = Direct::whole(text) {
        return Ok(v);
    }
    let value = parse(text).map_err(|e| Error::new(400, format!("Invalid JSON: {e}")))?;
    from_value(&value)
}

/// Reads a parsed value into a `T`, with [`from_json`]'s 422.
pub(crate) fn from_value<T: FromJson>(value: &Value) -> crate::Result<T> {
    let mut problems = Problems::default();
    match T::from_json(value, &mut problems) {
        Some(v) if problems.is_empty() => Ok(v),
        _ => Err(Error::invalid_fields(problems.list)),
    }
}

/// `value` as JSON text.
pub fn to_json(value: &(impl Json + ?Sized)) -> String {
    let mut out = String::new();
    value.json(&mut out);
    out
}

fn expected<T>(p: &mut Problems, what: &str, v: &Value) -> Option<T> {
    p.add(format!("expected {what}, found {}", v.kind()));
    None
}

impl FromJson for Value {
    fn from_json(v: &Value, _: &mut Problems) -> Option<Value> {
        Some(v.clone())
    }
}

impl FromJson for String {
    fn from_json(v: &Value, p: &mut Problems) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            other => expected(p, "a string", other),
        }
    }

    fn read(d: &mut Direct) -> Option<String> {
        d.string().map(String::from)
    }
}

impl FromJson for bool {
    fn from_json(v: &Value, p: &mut Problems) -> Option<bool> {
        match v {
            Value::Bool(b) => Some(*b),
            // A checkbox, as `input::flag` reads one.
            Value::String(s) if p.form => Some(crate::input::on(s)),
            other => expected(p, "true or false", other),
        }
    }

    /// Left out is `false`, as an unticked checkbox is in a form.
    fn missing() -> Option<bool> {
        Some(false)
    }

    fn read(d: &mut Direct) -> Option<bool> {
        d.boolean()
    }
}

impl FromJson for () {
    fn from_json(v: &Value, p: &mut Problems) -> Option<()> {
        match v {
            Value::Null => Some(()),
            other => expected(p, "null", other),
        }
    }

    fn read(d: &mut Direct) -> Option<()> {
        d.null().then_some(())
    }
}

macro_rules! integers {
    ($($t:ty)*) => {$(
        impl FromJson for $t {
            fn from_json(v: &Value, p: &mut Problems) -> Option<$t> {
                let n = match v {
                    Value::Number(n) => n.as_str(),
                    Value::String(s) if p.form => s.trim(),
                    _ => return expected(p, "a number", v),
                };
                let parsed = n.parse().ok();
                if parsed.is_none() {
                    let digits = n.strip_prefix(['-', '+']).unwrap_or(n);
                    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                        p.add("expected a whole number");
                    } else {
                        p.add(format!("must be from {} to {}", <$t>::MIN, <$t>::MAX));
                    }
                }
                parsed
            }

            fn read(d: &mut Direct) -> Option<$t> {
                d.number()?.parse().ok()
            }
        }
    )*};
}

integers!(u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize);

macro_rules! floats {
    ($($t:ty)*) => {$(
        impl FromJson for $t {
            fn from_json(v: &Value, p: &mut Problems) -> Option<$t> {
                match v {
                    // Grammar-checked text always parses; a huge one is infinite.
                    Value::Number(n) => n.parse().ok().filter(|f: &$t| f.is_finite()).or_else(|| {
                        p.add("is too large");
                        None
                    }),
                    Value::String(s) if p.form => s.trim().parse().ok().filter(|f: &$t| f.is_finite()).or_else(|| {
                        p.add("expected a number");
                        None
                    }),
                    other => expected(p, "a number", other),
                }
            }

            fn read(d: &mut Direct) -> Option<$t> {
                d.number()?.parse().ok().filter(|f: &$t| f.is_finite())
            }
        }
    )*};
}

floats!(f32 f64);

impl<T: FromJson> FromJson for Option<T> {
    fn from_json(v: &Value, p: &mut Problems) -> Option<Option<T>> {
        match v {
            Value::Null => Some(None),
            v => T::from_json(v, p).map(Some),
        }
    }

    fn missing() -> Option<Option<T>> {
        Some(None)
    }

    const SEALS: bool = T::SEALS;

    fn seal(&mut self, hash: bool) {
        if let Some(v) = self {
            v.seal(hash);
        }
    }

    fn read(d: &mut Direct) -> Option<Option<T>> {
        match d.null() {
            true => Some(None),
            false => T::read(d).map(Some),
        }
    }
}

impl<T: FromJson> FromJson for Box<T> {
    fn from_json(v: &Value, p: &mut Problems) -> Option<Box<T>> {
        T::from_json(v, p).map(Box::new)
    }

    const SEALS: bool = T::SEALS;

    fn seal(&mut self, hash: bool) {
        (**self).seal(hash);
    }

    fn read(d: &mut Direct) -> Option<Box<T>> {
        T::read(d).map(Box::new)
    }
}

impl<T: FromJson> FromJson for Vec<T> {
    fn from_json(v: &Value, p: &mut Problems) -> Option<Vec<T>> {
        let items = match v {
            Value::Array(items) => items.as_slice(),
            // A form's field sent once.
            Value::String(_) if p.form => std::slice::from_ref(v),
            _ => return expected(p, "an array", v),
        };
        // Every item is read, so each bad one is reported.
        let mut out = Vec::with_capacity(items.len());
        let mut ok = true;
        for (i, item) in items.iter().enumerate() {
            match p.item(i, item) {
                Some(x) if ok => out.push(x),
                Some(_) => {}
                None => ok = false,
            }
        }
        ok.then_some(out)
    }

    const SEALS: bool = T::SEALS;

    fn seal(&mut self, hash: bool) {
        self.iter_mut().for_each(|v| v.seal(hash));
    }

    /// Left out is empty, as a form's repeated field sent no times is.
    fn missing() -> Option<Vec<T>> {
        Some(Vec::new())
    }

    fn read(d: &mut Direct) -> Option<Vec<T>> {
        d.array()?;
        let (mut out, mut first) = (Vec::new(), true);
        while d.item(&mut first)? {
            out.push(T::read(d)?);
        }
        Some(out)
    }
}

/// The members of an object, each read as a `T`.
fn members<T: FromJson>(v: &Value, p: &mut Problems) -> Option<Vec<(String, T)>> {
    let members = p.object(v)?;
    let mut out = Vec::with_capacity(members.len());
    let mut ok = true;
    for (k, item) in members {
        match p.read(k, item) {
            Some(x) if ok => out.push((k.clone(), x)),
            Some(_) => {}
            None => ok = false,
        }
    }
    ok.then_some(out)
}

impl<T: FromJson> FromJson for BTreeMap<String, T> {
    fn from_json(v: &Value, p: &mut Problems) -> Option<BTreeMap<String, T>> {
        members(v, p).map(|m| m.into_iter().collect())
    }
}

impl<T: FromJson, S: BuildHasher + Default> FromJson for HashMap<String, T, S> {
    fn from_json(v: &Value, p: &mut Problems) -> Option<HashMap<String, T, S>> {
        members(v, p).map(|m| m.into_iter().collect())
    }
}

/// The checks `#[validate(...)]` runs. Each returns the problem, if any.
/// A field that is `None` passes: whether it may be left out is its type's
/// business.
pub mod check {
    /// A number, for `min` and `max`.
    pub trait Number {
        fn number(&self) -> Option<f64>;
    }

    /// Something with a length, for `min_len` and `max_len`: a string in
    /// characters, a list in items.
    pub trait Length {
        fn length(&self) -> Option<(usize, &'static str)>;
    }

    /// Text, for `email`.
    pub trait Text {
        fn text(&self) -> Option<&str>;
    }

    macro_rules! numbers {
        ($($t:ty)*) => {$(
            impl Number for $t {
                fn number(&self) -> Option<f64> {
                    Some(*self as f64)
                }
            }
        )*};
    }

    numbers!(u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize f32 f64);

    impl<T: Number> Number for Option<T> {
        fn number(&self) -> Option<f64> {
            self.as_ref()?.number()
        }
    }

    impl Length for String {
        fn length(&self) -> Option<(usize, &'static str)> {
            Some((self.chars().count(), "character"))
        }
    }

    impl Length for crate::Password {
        fn length(&self) -> Option<(usize, &'static str)> {
            Some((self.chars().count(), "character"))
        }
    }

    impl<T> Length for Vec<T> {
        fn length(&self) -> Option<(usize, &'static str)> {
            Some((self.len(), "item"))
        }
    }

    impl<T: Length> Length for Option<T> {
        fn length(&self) -> Option<(usize, &'static str)> {
            self.as_ref()?.length()
        }
    }

    impl Text for String {
        fn text(&self) -> Option<&str> {
            Some(self)
        }
    }

    impl<T: Text> Text for Option<T> {
        fn text(&self) -> Option<&str> {
            self.as_ref()?.text()
        }
    }

    fn plural(n: usize, unit: &str) -> String {
        if n == 1 {
            format!("1 {unit}")
        } else {
            format!("{n} {unit}s")
        }
    }

    pub fn min(v: &impl Number, min: f64) -> Option<String> {
        // `NaN` (a form says "NaN" to an `f64`) is below nothing and above
        // nothing: it must not pass for a number in range.
        let n = v.number()?;
        (n.is_nan() || n < min).then(|| format!("must be at least {min}"))
    }

    pub fn max(v: &impl Number, max: f64) -> Option<String> {
        let n = v.number()?;
        (n.is_nan() || n > max).then(|| format!("must be at most {max}"))
    }

    pub fn min_len(v: &impl Length, min: usize) -> Option<String> {
        let (n, unit) = v.length()?;
        (n < min).then(|| format!("must have at least {}", plural(min, unit)))
    }

    pub fn max_len(v: &impl Length, max: usize) -> Option<String> {
        let (n, unit) = v.length()?;
        (n > max).then(|| format!("must have at most {}", plural(max, unit)))
    }

    /// An address as `<input type="email">` takes one, so the server never
    /// refuses what a browser sent: WHATWG's "valid email address". Before
    /// the `@`, letters, digits and ``.!#$%&'*+/=?^_`{|}~-``; after it,
    /// labels between dots, each letters, digits and `-` but not at either
    /// end (`a@b` is one). A label may also hold non-ASCII letters and be
    /// longer than 63, as Firefox sends them (it checks the punycode).
    /// Only an email that arrives proves an address.
    pub fn email(v: &impl Text) -> Option<String> {
        let s = v.text()?;
        let user_byte = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+/=?^_`{|}~.-".contains(&b);
        let label_end = |b: &u8| b.is_ascii_alphanumeric() || *b >= 0x80;
        let label = |l: &str| {
            let b = l.as_bytes();
            b.first().is_some_and(label_end)
                && b.last().is_some_and(label_end)
                && b.iter().all(|c| label_end(c) || *c == b'-')
        };
        let ok = s.split_once('@').is_some_and(|(user, domain)| {
            !user.is_empty() && user.bytes().all(user_byte) && domain.split('.').all(label)
        });
        (!ok).then(|| "must be an email address".to_string())
    }

    /// An absolute `http` or `https` address: a host after the scheme, and
    /// no spaces or control characters.
    pub fn url(v: &impl Text) -> Option<String> {
        let s = v.text()?;
        let host = s
            .strip_prefix("https://")
            .or_else(|| s.strip_prefix("http://"))
            .and_then(|r| r.split(['/', '?', '#']).next())
            .filter(|h| !h.is_empty() && !h.starts_with(['@', ':']));
        let clean = !s.chars().any(|c| c.is_whitespace() || c.is_control());
        (host.is_none() || !clean).then(|| "must be a web address".to_string())
    }

    /// One of the words of `words`, separated by spaces.
    pub fn one_of(v: &impl Text, words: &str) -> Option<String> {
        let s = v.text()?;
        let ok = words.split_whitespace().any(|w| w == s);
        (!ok).then(|| {
            let all: Vec<_> = words.split_whitespace().collect();
            format!("must be one of {}", all.join(", "))
        })
    }

    /// The whole text matches `pattern`, as `<input pattern>` reads one,
    /// for the part of it that is common: literals, `.`, classes (`[a-z_]`,
    /// `[^x]`, `\d \w \s` and their capitals), groups, `|`, and `? * + {n}
    /// {n,m}`. Longer than 1000 characters, or too much backtracking, does
    /// not match.
    pub fn pattern(v: &impl Text, pattern: &str) -> Option<String> {
        let s = v.text()?;
        let Some(alts) = wisp_shared::pattern::parse(pattern) else {
            return Some("has a pattern that cannot be read".into());
        };
        (!wisp_shared::pattern::matches(&alts, s)).then(|| "does not match the pattern".to_string())
    }

    /// A pattern read once: what a rule's check keeps, one to each, so the
    /// request does not read it again.
    #[doc(hidden)]
    pub struct Pattern(std::sync::OnceLock<Option<wisp_shared::pattern::Alts>>);

    impl Pattern {
        pub const fn new() -> Pattern {
            Pattern(std::sync::OnceLock::new())
        }
    }

    impl Default for Pattern {
        fn default() -> Pattern {
            Pattern::new()
        }
    }

    /// [`pattern`] for `once`, which is for this `pattern` only.
    #[doc(hidden)]
    pub fn pattern_once(v: &impl Text, once: &Pattern, pattern: &str) -> Option<String> {
        let s = v.text()?;
        let Some(alts) = once.0.get_or_init(|| wisp_shared::pattern::parse(pattern)) else {
            return Some("has a pattern that cannot be read".into());
        };
        (!wisp_shared::pattern::matches(alts, s)).then(|| "does not match the pattern".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nan_is_in_no_range() {
        assert!(check::min(&f64::NAN, 0.0).is_some() && check::max(&f64::NAN, 9.0).is_some());
        assert!(check::min(&1.0, 0.0).is_none() && check::max(&Some(f64::NAN), 9.0).is_some());
    }

    #[test]
    fn parses_strictly() {
        let v =
            parse(" {\"a\": [1, -2.5e3, true, null, \"x\\u00e9\\ud83d\\ude00\\n\"], \"a\": 0} ")
                .unwrap();
        assert_eq!(
            v.get("a"),
            Some(&Value::Number("0".into())),
            "the last key counts"
        );
        let Value::Object(m) = &v else { panic!() };
        assert_eq!(
            m[0].1,
            Value::Array(vec![
                Value::Number("1".into()),
                Value::Number("-2.5e3".into()),
                Value::Bool(true),
                Value::Null,
                Value::String("xé😀\n".into()),
            ])
        );
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1.",
            "-",
            ".5",
            "+1",
            "tru",
            "nul",
            "'a'",
            "{a:1}",
            "[1 2]",
            "\"\\x\"",
            "\"a\nb\"",
            "\"\\ud800\"",
            "1 2",
            "// c\n1",
            "NaN",
            "1e",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} is not JSON");
        }
        assert_eq!(
            parse("[1,\n  x]").unwrap_err(),
            "expected a value at line 2, column 3"
        );
        let deep = "[".repeat(200) + &"]".repeat(200);
        assert_eq!(
            parse(&deep).unwrap_err(),
            "nested too deeply at line 1, column 129"
        );
        assert_eq!(
            parse(&("[".repeat(100) + &"]".repeat(100))).map(|_| ()),
            Ok(())
        );
    }

    use crate::fuzz::{Rng, mutate};

    fn any_value(rng: &mut Rng, depth: u32) -> Value {
        match rng.below(if depth > 4 { 4 } else { 6 }) {
            0 => Value::Null,
            1 => Value::Bool(rng.one_in(2)),
            2 => Value::Number(
                rng.pick(&[
                    "0",
                    "-0",
                    "7",
                    "-12.5e-3",
                    "1E+400",
                    "18446744073709551616",
                    "0.1",
                ])
                .into(),
            ),
            3 => Value::String(rng.text(12)),
            4 => Value::Array(
                (0..rng.below(4))
                    .map(|_| any_value(rng, depth + 1))
                    .collect(),
            ),
            _ => Value::Object(
                (0..rng.below(4))
                    .map(|_| (rng.text(6), any_value(rng, depth + 1)))
                    .collect(),
            ),
        }
    }

    #[test]
    fn any_value_reads_back_and_broken_text_never_panics() {
        let mut rng = Rng::new(30);
        for _ in 0..5000 {
            let v = any_value(&mut rng, 0);
            let text = to_json(&v);
            assert_eq!(parse(&text).as_ref(), Ok(&v), "{text}");
            let spaced = format!(" \n{text}\t\r");
            assert_eq!(parse(&spaced).as_ref(), Ok(&v), "{spaced}");
            for _ in 0..4 {
                let mut broken = text.clone().into_bytes();
                mutate(&mut rng, &mut broken);
                if let Ok(text) = std::str::from_utf8(&broken) {
                    let _ = parse(text);
                }
                let _ = from_json::<Vec<BTreeMap<String, Option<i64>>>>(&broken);
            }
        }
        // Deep nesting is refused, not a stack overflow, however it is mixed.
        let deep = "[{\"a\":".repeat(10_000);
        assert!(parse(&deep).unwrap_err().starts_with("nested too deeply"));
    }

    #[test]
    fn writes_back() {
        let text = "{\"a\":[1,-2.5e3,true,null,\"x<\"],\"b\":{}}";
        assert_eq!(to_json(&parse(text).unwrap()), text.replace('<', "\\u003c"));
    }

    fn read<T: FromJson>(text: &str) -> crate::Result<T> {
        from_json(text.as_bytes())
    }

    #[test]
    fn reads_types() {
        assert_eq!(read::<Vec<u8>>("[1, 2]").unwrap(), [1, 2]);
        assert_eq!(read::<Option<String>>("null").unwrap(), None);
        assert_eq!(read::<Option<String>>("").unwrap(), None, "no body");
        assert_eq!(read::<i64>("-9223372036854775808").unwrap(), i64::MIN);
        assert_eq!(read::<f64>("1e2").unwrap(), 100.0);
        let m: BTreeMap<String, bool> = read("{\"x\": true}").unwrap();
        assert!(m["x"]);
        let e = read::<Vec<u8>>("[1, 300, \"a\", 2.5]").unwrap_err();
        assert_eq!(e.status(), 422);
        assert_eq!(
            e.fields(),
            [
                ("[1]".to_string(), "must be from 0 to 255".to_string()),
                ("[2]".into(), "expected a number, found a string".into()),
                ("[3]".into(), "expected a whole number".into()),
            ]
        );
        assert_eq!(read::<String>("{").unwrap_err().status(), 400);
        assert_eq!(
            read::<String>("").unwrap_err().message(),
            "Expected a JSON body"
        );
        assert_eq!(
            read::<f32>("1e999").unwrap_err().fields()[0].1,
            "is too large"
        );
    }

    #[test]
    fn a_flood_of_problems_is_cut_short() {
        // Each problem was checked against every one before it: 300,000
        // bad items took minutes, and the answer was megabytes.
        let many = format!("[{}1]", "1,".repeat(300_000));
        let e = read::<Vec<String>>(&many).unwrap_err();
        assert_eq!((e.status(), e.fields().len()), (422, MOST));
        let long = format!("{{\"{}\":[1,1,1]}}", "k".repeat(100_000));
        let e = read::<BTreeMap<String, Vec<String>>>(&long).unwrap_err();
        assert_eq!(e.fields().len(), 1, "past the bytes, no more");
    }

    #[test]
    fn fields_and_checks() {
        let v = parse("{\"a\": {\"b\": 1}, \"n\": 5}").unwrap();
        let mut p = Problems::default();
        let m = p.object(&v).unwrap();
        assert_eq!(p.field::<Option<u8>>(m, "gone"), Some(None));
        assert_eq!(p.field::<u8>(m, "gone"), None);
        let a = p.object(m[0].1.get("b").unwrap());
        assert!(a.is_none());
        let n: u8 = p.field(m, "n").unwrap();
        p.check("n", check::max(&n, 3.0));
        p.check("n", check::min(&n, 0.0));
        assert_eq!(
            p.list,
            [
                ("gone".to_string(), "is required".to_string()),
                ("body".into(), "expected an object, found a number".into()),
                ("n".into(), "must be at most 3".into()),
            ]
        );
        assert_eq!(
            check::min_len(&"é".to_string(), 2).as_deref(),
            Some("must have at least 2 characters")
        );
        assert_eq!(
            check::max_len(&vec![1, 2], 1).as_deref(),
            Some("must have at most 1 item")
        );
        assert_eq!(check::min_len(&None::<String>, 1), None);
        // What `<input type="email">` takes, and only that (but Firefox's
        // Unicode and long labels).
        let long = format!("a@{}.com", "b".repeat(70));
        for good in [
            "a@b.co",
            "a@b",
            "first.last+tag@mail.example.org",
            ".a..b.@c",
            "!#$%&'*+/=?^_`{|}~-@x",
            "a@1.2.3.4",
            "a@a-b--c.d",
            "a@xn--bcher-kva.de",
            "a@bücher.de",
            &long,
        ] {
            assert_eq!(check::email(&good.to_string()), None, "{good}");
        }
        for bad in [
            "",
            "a",
            "@b.co",
            "a@",
            "a@b.",
            "a@.b",
            "a@b..c",
            "a@-b",
            "a@b-",
            "a@b-.c",
            "a@@b.co",
            "a@b@c",
            "a b@c.de",
            "a@b c",
            " a@b",
            "a@b\n",
            "a\"b@c",
            "(a)@b",
            "a,b@c",
            "ü@b",
            "a@b_c",
            "a@[1.2.3.4]",
            "a@b:80",
        ] {
            assert!(check::email(&bad.to_string()).is_some(), "{bad}");
        }
    }

    #[test]
    fn url_one_of_and_pattern() {
        let t = |s: &str| s.to_string();
        for ok in ["http://a.b", "https://a.b/c?d#e", "http://localhost:80"] {
            assert!(check::url(&t(ok)).is_none(), "{ok}");
        }
        for bad in [
            "",
            "a.b",
            "ftp://a.b",
            "http://",
            "https:///x",
            "http://a b",
            "http://@x",
        ] {
            assert!(check::url(&t(bad)).is_some(), "{bad}");
        }
        assert!(check::one_of(&t("b"), "a b c").is_none());
        assert_eq!(
            check::one_of(&t("d"), "a b c").as_deref(),
            Some("must be one of a, b, c")
        );
        assert!(check::one_of(&None::<String>, "a").is_none());
        for (pat, yes, no) in [
            ("[a-z]+", "abc", "ab1"),
            (r"\d{3}-\d{4}", "555-1234", "55-1234"),
            ("(red|green|blue)s?", "greens", "grey"),
            ("[^x]*", "abc", "axb"),
            ("a.c", "a-c", "ac"),
            (r"[\w.]+@x", "a.b_c@x", "a b@x"),
            ("(ab)*c", "ababc", "abac"),
            ("x{2,}", "xxx", "x"),
            ("", "", "a"),
            (r"\.", ".", "a"),
            ("(a*)*b", "aab", "aac"),
        ] {
            assert!(check::pattern(&t(yes), pat).is_none(), "{pat} {yes}");
            assert!(check::pattern(&t(no), pat).is_some(), "{pat} {no}");
        }
        for bad in ["(a", "a)", "[a", "*a", "a{x}", "[z-a]", "a{1", r"[\D]"] {
            assert_eq!(
                check::pattern(&t("a"), bad).as_deref(),
                Some("has a pattern that cannot be read"),
                "{bad}"
            );
        }
        // Too much backtracking, or too long, never hangs: it does not match.
        let slow = "a".repeat(40) + "!";
        assert!(check::pattern(&slow, "(a|aa)*b").is_some());
        assert!(check::pattern(&"a".repeat(2000), "a*").is_some());
    }
}
