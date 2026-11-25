//! A reader for Mozilla's preference files (`prefs.js`, `user.js`), after
//! the grammar of Gecko's own parser (modules/libpref/parser):
//!
//! ```text
//! pref      = spec "(" string "," value ("," attr)* ")" ";"
//! spec      = "user_pref" | "pref" | "sticky_pref"
//! value     = string | "true" | "false" | int
//! int       = ["+" | "-"] digit+            (a 32-bit signed number)
//! attr      = "sticky" | "locked"
//! string    = '"' … '"' | "'" … "'"         (escapes \\ \" \' \n \r \xHH \uHHHH)
//! ```
//!
//! Comments are `//` and `#` to the end of the line, and `/* … */`. A pref
//! that does not parse is skipped to the next `;` and noted by its line,
//! never by what it held; the others are kept. A later value for a name
//! replaces an earlier one, as in Thunderbird.

use std::collections::HashMap;

/// A preference's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefValue {
    String(String),
    Int(i32),
    Bool(bool),
}

/// A pref that did not parse: where, and what was wrong, never what it
/// held (a pref may hold anything).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefError {
    /// The line it starts on, from 1.
    pub line: usize,
    pub what: &'static str,
}

/// The preferences read from one or more files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prefs {
    values: HashMap<String, PrefValue>,
    /// The prefs that did not parse.
    pub errors: Vec<PrefError>,
}

impl Prefs {
    /// The prefs in `text`.
    pub fn parse(text: &str) -> Self {
        let mut prefs = Self::default();
        prefs.read(text);
        prefs
    }

    /// Reads `text` over what is here: a pref it sets replaces the one
    /// here (as `user.js` does `prefs.js`'s). Its errors are added.
    pub fn read(&mut self, text: &str) {
        let mut parser = Parser {
            text,
            pos: 0,
            line: 1,
        };
        loop {
            if let Err(what) = parser.skip_blank() {
                self.errors.push(PrefError {
                    line: parser.line,
                    what,
                });
                break;
            }
            if parser.peek().is_none() {
                break;
            }
            let line = parser.line;
            match parser.pref() {
                Ok((name, value)) => {
                    self.values.insert(name, value);
                }
                Err(what) => {
                    self.errors.push(PrefError { line, what });
                    parser.recover();
                }
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&PrefValue> {
        self.values.get(name)
    }

    /// The pref's text, if it is a string.
    pub fn string(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            PrefValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// The pref's number, if it is an int.
    pub fn int(&self, name: &str) -> Option<i32> {
        match self.get(name)? {
            PrefValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// The pref's truth, if it is a bool.
    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            PrefValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The names of the prefs held, in no order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }
}

type Parsed<T> = Result<T, &'static str>;

struct Parser<'a> {
    text: &'a str,
    /// Where it is, in bytes.
    pos: usize,
    line: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        if c == '\n' {
            self.line += 1;
        }
        Some(c)
    }

    /// Skips white space and comments.
    fn skip_blank(&mut self) -> Parsed<()> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('#') => self.skip_line(),
                Some('/') if self.rest().starts_with("//") => self.skip_line(),
                Some('/') if self.rest().starts_with("/*") => {
                    self.bump();
                    self.bump();
                    loop {
                        if self.rest().starts_with("*/") {
                            self.bump();
                            self.bump();
                            break;
                        }
                        if self.bump().is_none() {
                            return Err("a comment is not closed");
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn skip_line(&mut self) {
        while let Some(c) = self.bump() {
            if c == '\n' {
                break;
            }
        }
    }

    /// Skips past the next `;`, after a pref that did not parse.
    fn recover(&mut self) {
        while let Some(c) = self.bump() {
            if c == ';' {
                break;
            }
        }
    }

    fn word(&mut self) -> &str {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            self.bump();
        }
        &self.text[start..self.pos]
    }

    fn expect(&mut self, want: char, what: &'static str) -> Parsed<()> {
        self.skip_blank()?;
        if self.peek() == Some(want) {
            self.bump();
            Ok(())
        } else {
            Err(what)
        }
    }

    fn pref(&mut self) -> Parsed<(String, PrefValue)> {
        if !matches!(self.word(), "user_pref" | "pref" | "sticky_pref") {
            return Err("not a pref");
        }
        self.expect('(', "a pref has no “(”")?;
        self.skip_blank()?;
        if !matches!(self.peek(), Some('"' | '\'')) {
            return Err("a pref's name is not a string");
        }
        let name = self.string()?;
        self.expect(',', "a pref has no value")?;
        self.skip_blank()?;
        let value = self.value()?;
        loop {
            self.skip_blank()?;
            // Peeked, not taken: a `;` here is where recovery stops.
            match self.peek() {
                Some(')') => {
                    self.bump();
                    break;
                }
                Some(',') => {
                    self.bump();
                    self.skip_blank()?;
                    if !matches!(self.word(), "sticky" | "locked") {
                        return Err("a pref's attribute is not sticky or locked");
                    }
                }
                _ => return Err("a pref is not closed with “)”"),
            }
        }
        self.expect(';', "a pref does not end with “;”")?;
        Ok((name, value))
    }

    fn value(&mut self) -> Parsed<PrefValue> {
        match self.peek() {
            Some('"' | '\'') => Ok(PrefValue::String(self.string()?)),
            Some('+' | '-') | Some('0'..='9') => self.int(),
            Some(c) if c.is_ascii_alphabetic() => match self.word() {
                "true" => Ok(PrefValue::Bool(true)),
                "false" => Ok(PrefValue::Bool(false)),
                _ => Err("a pref's value is not a string, number, true or false"),
            },
            _ => Err("a pref's value is not a string, number, true or false"),
        }
    }

    fn int(&mut self) -> Parsed<PrefValue> {
        let negative = match self.peek() {
            Some('-') => {
                self.bump();
                true
            }
            Some('+') => {
                self.bump();
                false
            }
            _ => false,
        };
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        let digits = &self.text[start..self.pos];
        if digits.is_empty() {
            return Err("a number has no digits");
        }
        if self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err("a number runs into letters");
        }
        let magnitude: i64 = digits.parse().map_err(|_| "a number is too big")?;
        let n = if negative { -magnitude } else { magnitude };
        i32::try_from(n)
            .map(PrefValue::Int)
            .map_err(|_| "a number is too big")
    }

    /// A quoted string; the quote it opens with closes it.
    fn string(&mut self) -> Parsed<String> {
        let quote = self.bump().ok_or("a string is missing")?;
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return Err("a string is not closed"),
                Some(c) if c == quote => return Ok(s),
                Some('\\') => s.push(self.escape()?),
                Some(c) => s.push(c),
            }
        }
    }

    /// The character an escape (after its `\`) stands for.
    fn escape(&mut self) -> Parsed<char> {
        match self.bump() {
            Some(c @ ('"' | '\'' | '\\')) => Ok(c),
            Some('n') => Ok('\n'),
            Some('r') => Ok('\r'),
            Some('x') => match self.hex(2)? {
                0 => Err("a string holds \\x00"),
                n => char::from_u32(n).ok_or("a string holds a bad \\x escape"),
            },
            Some('u') => self.unicode(),
            _ => Err("a string holds an unknown escape"),
        }
    }

    /// `\uHHHH`, with a surrogate pair written as two of them.
    fn unicode(&mut self) -> Parsed<char> {
        let first = self.hex(4)?;
        match first {
            0 => Err("a string holds \\u0000"),
            0xD800..=0xDBFF => {
                if !self.rest().starts_with("\\u") {
                    return Err("a string holds half a surrogate pair");
                }
                self.bump();
                self.bump();
                let second = self.hex(4)?;
                if !(0xDC00..=0xDFFF).contains(&second) {
                    return Err("a string holds half a surrogate pair");
                }
                let c = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                char::from_u32(c).ok_or("a string holds a bad \\u escape")
            }
            0xDC00..=0xDFFF => Err("a string holds half a surrogate pair"),
            n => char::from_u32(n).ok_or("a string holds a bad \\u escape"),
        }
    }

    fn hex(&mut self, digits: usize) -> Parsed<u32> {
        let mut n = 0;
        for _ in 0..digits {
            let d = self
                .bump()
                .and_then(|c| c.to_digit(16))
                .ok_or("a string holds a bad hex escape")?;
            n = n * 16 + d;
        }
        Ok(n)
    }
}

#[cfg(test)]
#[path = "prefs_tests.rs"]
mod tests;
