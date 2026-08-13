//! Keyfire formula language — small expression evaluator for `{=expr}` and
//! `{if expr}…{endif}` tokens.
//!
//! Grammar (precedence low → high):
//!   or       := and ('||' and)*
//!   and      := compare ('&&' compare)*
//!   compare  := concat (('==' | '!=' | '<' | '>' | '<=' | '>=') concat)?
//!   concat   := additive ('&' additive)*
//!   additive := mult (('+' | '-') mult)*
//!   mult     := unary (('*' | '/' | '%') unary)*
//!   unary    := ('!' | '-') unary | primary
//!   primary  := NUMBER | STRING | IDENT | IDENT '(' args? ')' | '(' or ')'
//!   args     := or (',' or)*
//!
//! Values are loosely typed (`String / Number / Bool`) with implicit coercion
//! where Text Blaze does the same (arithmetic coerces to number, `&` concat
//! coerces to string, comparisons try number then fall back to string lex
//! compare). Mismatch errors come back to the caller as `Err(msg)`; the
//! integration layer in `expansions.rs` renders them inline as `«error: msg»`
//! so a single broken formula never kills the whole expansion fire.
//!
//! Scope lookup order for bare identifiers: fill-in values → global vars →
//! reserved (`selection`, `clipboard`, `yes`/`true`, `no`/`false`) → error.
//! Function names use the same identifier slot but only resolve in call
//! position (`upper(name)`), never as a bare value.

use chrono::{Datelike, Local, NaiveDate};
use std::collections::HashMap;

// ── Value ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum Value {
    String(String),
    Number(f64),
    Bool(bool),
}

impl Value {
    pub fn to_display_string(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Number(n) => format_number(*n),
            Value::Bool(b) => if *b { "yes".into() } else { "no".into() },
        }
    }

    fn to_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::String(s) => {
                let t = s.trim();
                !t.is_empty()
                    && !t.eq_ignore_ascii_case("no")
                    && !t.eq_ignore_ascii_case("false")
                    && t != "0"
            }
        }
    }

    fn to_number(&self) -> Result<f64, String> {
        match self {
            Value::Number(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::String(s) => s
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("cannot convert '{}' to number", s)),
        }
    }

    fn to_string_coerced(&self) -> String {
        self.to_display_string()
    }
}

fn format_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    // Round to 2 decimal places for display. Currency / invoice work is the
    // dominant formula use case; raw float defaults like 3.141592653589793
    // are noisy. We round HERE so downstream `{=foo * 2}` references against
    // a stored set-var also pick up the rounded value (set-var values are
    // stored as their display string).
    //
    // Whole-number results render without a decimal point (`7`, not `7.00`)
    // and one-decimal results drop the trailing zero (`3.5`, not `3.50`).
    // That keeps integer outputs (e.g. datediff result in days) clean while
    // still capping noisy floats at 2 dp.
    let rounded = (n * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 && rounded.abs() < 1e16 {
        format!("{:.0}", rounded)
    } else {
        let s = format!("{:.2}", rounded);
        // `123.50` → `123.5`, but `123.45` stays. We only strip ONE trailing
        // zero — the `.00` case is caught by the integer branch above.
        if s.ends_with('0') {
            s[..s.len() - 1].to_string()
        } else {
            s
        }
    }
}

// ── Scope ──────────────────────────────────────────────────────────────────

/// Lookup tables an expression evaluates against. `local_vars` is owned and
/// mutable so the `{set name = expr}` scanner can populate it during a fire
/// and later `{=…}` / `{if …}` tokens see the result. Borrowed views are
/// kept for the unchanging maps to avoid cloning fill-ins / globals.
pub struct Scope<'a> {
    pub fillin_values: &'a HashMap<String, String>,
    pub global_vars: &'a HashMap<String, String>,
    pub local_vars: HashMap<String, String>,
    pub selection: &'a str,
    pub clipboard: &'a str,
}

impl<'a> Scope<'a> {
    /// Insert a value produced by `{set name = expr}` so subsequent formula
    /// tokens in the same fire can reference it. Lookup order is
    /// fill-in → local → global → reserved, so a set var can derive from a
    /// fill-in and still be reachable by later expressions.
    pub fn set_local(&mut self, name: String, value: String) {
        self.local_vars.insert(name, value);
    }

    fn lookup(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.fillin_values.get(name) {
            return Some(Value::String(v.clone()));
        }
        if let Some(v) = self.local_vars.get(name) {
            return Some(Value::String(v.clone()));
        }
        if let Some(v) = self.global_vars.get(name) {
            return Some(Value::String(v.clone()));
        }
        match name {
            "selection" => Some(Value::String(self.selection.to_string())),
            "clipboard" => Some(Value::String(self.clipboard.to_string())),
            "yes" | "true"  => Some(Value::Bool(true)),
            "no"  | "false" => Some(Value::Bool(false)),
            _ => None,
        }
    }
}

// ── Lexer ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Number(f64),
    Str(String),
    Ident(String),
    Plus, Minus, Star, Slash, Percent,
    Eq, NotEq, Lt, Gt, LtEq, GtEq,
    And, Or, Not,
    Amp,
    LParen, RParen, Comma,
    Eof,
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let bytes = src.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();

    while i < bytes.len() {
        let c = bytes[i];

        // Whitespace
        if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
            i += 1;
            continue;
        }

        // Strings: " ... " with \" \\ \n \r \t escapes
        if c == b'"' {
            i += 1;
            let mut s = String::new();
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    let esc = bytes[i + 1];
                    s.push(match esc {
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'\\' => '\\',
                        b'"' => '"',
                        other => other as char,
                    });
                    i += 2;
                } else {
                    s.push(bytes[i] as char);
                    i += 1;
                }
            }
            if i >= bytes.len() {
                return Err("unterminated string literal".into());
            }
            i += 1; // closing quote
            out.push(Tok::Str(s));
            continue;
        }

        // Numbers: digits with optional single dot
        if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let slice = &src[start..i];
            let n: f64 = slice
                .parse()
                .map_err(|_| format!("invalid number: {}", slice))?;
            out.push(Tok::Number(n));
            continue;
        }

        // Identifiers / keywords (ascii letters + digits + underscore, must start with letter or _)
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Ident(src[start..i].to_string()));
            continue;
        }

        // Multi-char operators
        if i + 1 < bytes.len() {
            let pair = &src[i..i + 2];
            let two = match pair {
                "==" => Some(Tok::Eq),
                "!=" => Some(Tok::NotEq),
                "<=" => Some(Tok::LtEq),
                ">=" => Some(Tok::GtEq),
                "&&" => Some(Tok::And),
                "||" => Some(Tok::Or),
                _ => None,
            };
            if let Some(tok) = two {
                out.push(tok);
                i += 2;
                continue;
            }
        }

        // Single-char operators / punctuation
        let one = match c {
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => Tok::Star,
            b'/' => Tok::Slash,
            b'%' => Tok::Percent,
            b'<' => Tok::Lt,
            b'>' => Tok::Gt,
            b'!' => Tok::Not,
            b'&' => Tok::Amp,
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b',' => Tok::Comma,
            _ => {
                // Dump every byte of the expression body so the user can see
                // hidden chars (NBSP, zero-width, control codes). Each byte
                // gets either its char rendering or a \xNN escape if it isn't
                // printable ASCII / common whitespace.
                let mut dump = String::new();
                for (idx, &b) in bytes.iter().enumerate() {
                    if idx == i { dump.push('['); }
                    if b == b'\t' { dump.push_str("\\t"); }
                    else if b == b'\n' { dump.push_str("\\n"); }
                    else if b == b'\r' { dump.push_str("\\r"); }
                    else if (0x20..0x7F).contains(&b) { dump.push(b as char); }
                    else { dump.push_str(&format!("\\x{:02X}", b)); }
                    if idx == i { dump.push(']'); }
                }
                return Err(format!("unexpected character '{}' at byte {} in expression: \"{}\"", c as char, i, dump));
            }
        };
        out.push(one);
        i += 1;
    }

    out.push(Tok::Eof);
    Ok(out)
}

/// Format a token in a way that's useful in user-facing parser errors.
/// Quotes string content, shows numbers and identifiers literally, and gives
/// readable names to operators / punctuation so the user can spot the typo.
fn describe_tok(t: &Tok) -> String {
    match t {
        Tok::Number(n) => format!("{}", n),
        Tok::Str(s)    => format!("\"{}\"", s),
        Tok::Ident(s)  => format!("'{}'", s),
        Tok::Eof       => "end".into(),
        Tok::LParen    => "'('".into(),
        Tok::RParen    => "')'".into(),
        Tok::Comma     => "','".into(),
        Tok::Plus      => "'+'".into(),
        Tok::Minus     => "'-'".into(),
        Tok::Star      => "'*'".into(),
        Tok::Slash     => "'/'".into(),
        Tok::Percent   => "'%'".into(),
        Tok::Lt        => "'<'".into(),
        Tok::Gt        => "'>'".into(),
        Tok::LtEq      => "'<='".into(),
        Tok::GtEq      => "'>='".into(),
        Tok::Eq        => "'=='".into(),
        Tok::NotEq     => "'!='".into(),
        Tok::And       => "'&&'".into(),
        Tok::Or        => "'||'".into(),
        Tok::Not       => "'!'".into(),
        Tok::Amp       => "'&'".into(),
    }
}

// ── AST ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum Expr {
    Number(f64),
    Str(String),
    Ident(String),
    Call(String, Vec<Expr>),
    BinOp(BinOp, Box<Expr>, Box<Expr>),
    UnOp(UnOp, Box<Expr>),
}

#[derive(Debug, Clone, Copy)]
enum BinOp { Add, Sub, Mul, Div, Mod, Concat, Eq, NotEq, Lt, Gt, LtEq, GtEq, And, Or }

#[derive(Debug, Clone, Copy)]
enum UnOp { Neg, Not }

// ── Parser ─────────────────────────────────────────────────────────────────

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok { &self.toks[self.pos] }
    fn advance(&mut self) -> Tok {
        let t = self.toks[self.pos].clone();
        self.pos += 1;
        t
    }
    fn eat(&mut self, want: &Tok) -> bool {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(want) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse(&mut self) -> Result<Expr, String> {
        let e = self.parse_or()?;
        if !matches!(self.peek(), Tok::Eof) {
            return Err(format!("unexpected token after expression: {}", describe_tok(self.peek())));
        }
        Ok(e)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Tok::Or) {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::BinOp(BinOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_compare()?;
        while matches!(self.peek(), Tok::And) {
            self.advance();
            let right = self.parse_compare()?;
            left = Expr::BinOp(BinOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_compare(&mut self) -> Result<Expr, String> {
        let left = self.parse_concat()?;
        let op = match self.peek() {
            Tok::Eq    => Some(BinOp::Eq),
            Tok::NotEq => Some(BinOp::NotEq),
            Tok::Lt    => Some(BinOp::Lt),
            Tok::Gt    => Some(BinOp::Gt),
            Tok::LtEq  => Some(BinOp::LtEq),
            Tok::GtEq  => Some(BinOp::GtEq),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let right = self.parse_concat()?;
            return Ok(Expr::BinOp(op, Box::new(left), Box::new(right)));
        }
        Ok(left)
    }

    fn parse_concat(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_additive()?;
        while matches!(self.peek(), Tok::Amp) {
            self.advance();
            let right = self.parse_additive()?;
            left = Expr::BinOp(BinOp::Concat, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_mult()?;
        loop {
            let op = match self.peek() {
                Tok::Plus  => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mult()?;
            left = Expr::BinOp(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_mult(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star    => BinOp::Mul,
                Tok::Slash   => BinOp::Div,
                Tok::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            left = Expr::BinOp(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Tok::Not => {
                self.advance();
                let inner = self.parse_unary()?;
                Ok(Expr::UnOp(UnOp::Not, Box::new(inner)))
            }
            Tok::Minus => {
                self.advance();
                let inner = self.parse_unary()?;
                Ok(Expr::UnOp(UnOp::Neg, Box::new(inner)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.advance() {
            Tok::Number(n) => Ok(Expr::Number(n)),
            Tok::Str(s)    => Ok(Expr::Str(s)),
            Tok::LParen    => {
                let e = self.parse_or()?;
                if !self.eat(&Tok::RParen) {
                    return Err("missing closing ')'".into());
                }
                Ok(e)
            }
            Tok::Ident(name) => {
                if matches!(self.peek(), Tok::LParen) {
                    self.advance(); // consume (
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Tok::RParen) {
                        args.push(self.parse_or()?);
                        while matches!(self.peek(), Tok::Comma) {
                            self.advance();
                            args.push(self.parse_or()?);
                        }
                    }
                    if !self.eat(&Tok::RParen) {
                        return Err(format!("missing closing ')' in call to {}", name));
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Ident(name))
                }
            }
            other => Err(format!("unexpected token in expression: {:?}", other)),
        }
    }
}

// ── Evaluator ──────────────────────────────────────────────────────────────

fn eval(expr: &Expr, scope: &Scope<'_>) -> Result<Value, String> {
    match expr {
        Expr::Number(n) => Ok(Value::Number(*n)),
        Expr::Str(s)    => Ok(Value::String(s.clone())),
        Expr::Ident(name) => scope
            .lookup(name)
            .ok_or_else(|| format!("unknown identifier '{}'", name)),
        Expr::UnOp(op, inner) => {
            let v = eval(inner, scope)?;
            match op {
                UnOp::Neg => Ok(Value::Number(-v.to_number()?)),
                UnOp::Not => Ok(Value::Bool(!v.to_bool())),
            }
        }
        Expr::BinOp(op, l, r) => eval_binop(*op, l, r, scope),
        Expr::Call(name, args) => {
            let vals: Vec<Value> = args
                .iter()
                .map(|a| eval(a, scope))
                .collect::<Result<Vec<_>, _>>()?;
            call_builtin(name, &vals)
        }
    }
}

fn eval_binop(op: BinOp, l: &Expr, r: &Expr, scope: &Scope<'_>) -> Result<Value, String> {
    // Short-circuit for && / ||
    match op {
        BinOp::And => {
            let lv = eval(l, scope)?;
            if !lv.to_bool() {
                return Ok(Value::Bool(false));
            }
            return Ok(Value::Bool(eval(r, scope)?.to_bool()));
        }
        BinOp::Or => {
            let lv = eval(l, scope)?;
            if lv.to_bool() {
                return Ok(Value::Bool(true));
            }
            return Ok(Value::Bool(eval(r, scope)?.to_bool()));
        }
        _ => {}
    }

    let lv = eval(l, scope)?;
    let rv = eval(r, scope)?;

    match op {
        BinOp::Add => Ok(Value::Number(lv.to_number()? + rv.to_number()?)),
        BinOp::Sub => Ok(Value::Number(lv.to_number()? - rv.to_number()?)),
        BinOp::Mul => Ok(Value::Number(lv.to_number()? * rv.to_number()?)),
        BinOp::Div => {
            let r = rv.to_number()?;
            if r == 0.0 {
                return Err("division by zero".into());
            }
            Ok(Value::Number(lv.to_number()? / r))
        }
        BinOp::Mod => {
            let r = rv.to_number()?;
            if r == 0.0 {
                return Err("modulo by zero".into());
            }
            Ok(Value::Number(lv.to_number()? % r))
        }
        BinOp::Concat => Ok(Value::String(format!(
            "{}{}",
            lv.to_string_coerced(),
            rv.to_string_coerced()
        ))),
        BinOp::Eq    => Ok(Value::Bool(values_equal(&lv, &rv))),
        BinOp::NotEq => Ok(Value::Bool(!values_equal(&lv, &rv))),
        BinOp::Lt    => Ok(Value::Bool(values_cmp(&lv, &rv) == std::cmp::Ordering::Less)),
        BinOp::Gt    => Ok(Value::Bool(values_cmp(&lv, &rv) == std::cmp::Ordering::Greater)),
        BinOp::LtEq  => Ok(Value::Bool(values_cmp(&lv, &rv) != std::cmp::Ordering::Greater)),
        BinOp::GtEq  => Ok(Value::Bool(values_cmp(&lv, &rv) != std::cmp::Ordering::Less)),
        BinOp::And | BinOp::Or => unreachable!(),
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    // Try numeric first when both sides look numeric; otherwise compare as strings.
    if let (Ok(an), Ok(bn)) = (a.to_number(), b.to_number()) {
        return an == bn;
    }
    a.to_string_coerced() == b.to_string_coerced()
}

fn values_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    if let (Ok(an), Ok(bn)) = (a.to_number(), b.to_number()) {
        return an.partial_cmp(&bn).unwrap_or(std::cmp::Ordering::Equal);
    }
    a.to_string_coerced().cmp(&b.to_string_coerced())
}

// ── Built-in functions ─────────────────────────────────────────────────────

fn arg<'a>(args: &'a [Value], idx: usize, name: &str) -> Result<&'a Value, String> {
    args.get(idx)
        .ok_or_else(|| format!("{}: missing argument {}", name, idx + 1))
}

fn call_builtin(name: &str, args: &[Value]) -> Result<Value, String> {
    match name {
        // ── String functions ────────────────────────────────────────
        "upper" => Ok(Value::String(arg(args, 0, name)?.to_string_coerced().to_uppercase())),
        "lower" => Ok(Value::String(arg(args, 0, name)?.to_string_coerced().to_lowercase())),
        "trim"  => Ok(Value::String(arg(args, 0, name)?.to_string_coerced().trim().to_string())),
        "len"   => Ok(Value::Number(arg(args, 0, name)?.to_string_coerced().chars().count() as f64)),
        "substring" => {
            let s = arg(args, 0, name)?.to_string_coerced();
            let start = arg(args, 1, name)?.to_number()? as i64;
            let len_opt = args.get(2);
            let chars: Vec<char> = s.chars().collect();
            // Text Blaze-style: 1-based indexing; negative start is from end.
            let total = chars.len() as i64;
            let real_start = if start >= 0 { (start - 1).max(0) } else { (total + start).max(0) };
            let from = real_start as usize;
            let to = match len_opt {
                Some(v) => {
                    let n = v.to_number()? as i64;
                    let end = (real_start + n.max(0)).min(total) as usize;
                    end
                }
                None => chars.len(),
            };
            Ok(Value::String(chars[from.min(chars.len())..to.max(from)].iter().collect()))
        }
        "replace" => {
            let s = arg(args, 0, name)?.to_string_coerced();
            let find = arg(args, 1, name)?.to_string_coerced();
            let repl = arg(args, 2, name)?.to_string_coerced();
            if find.is_empty() {
                return Ok(Value::String(s));
            }
            Ok(Value::String(s.replace(&find, &repl)))
        }
        "contains" => {
            let s = arg(args, 0, name)?.to_string_coerced();
            let v = arg(args, 1, name)?.to_string_coerced();
            Ok(Value::Bool(s.contains(&v)))
        }
        "startswith" => {
            let s = arg(args, 0, name)?.to_string_coerced();
            let v = arg(args, 1, name)?.to_string_coerced();
            Ok(Value::Bool(s.starts_with(&v)))
        }
        "endswith" => {
            let s = arg(args, 0, name)?.to_string_coerced();
            let v = arg(args, 1, name)?.to_string_coerced();
            Ok(Value::Bool(s.ends_with(&v)))
        }
        "urlencode" => Ok(Value::String(url_encode(&arg(args, 0, name)?.to_string_coerced()))),

        // ── Date functions ──────────────────────────────────────────
        // Dates are passed around as YYYY-MM-DD ISO strings (the format the
        // HTML5 date input emits and `{fillIn:Label:date}` stores). Combine
        // with `dateformat` to render a final localised representation.
        "today" => {
            let t = Local::now().date_naive();
            Ok(Value::String(t.format("%Y-%m-%d").to_string()))
        }
        "dateadd" => {
            let date_str = arg(args, 0, name)?.to_string_coerced();
            let days = arg(args, 1, name)?.to_number()? as i64;
            let parsed = parse_iso_date(&date_str)
                .ok_or_else(|| format!("dateadd: cannot parse '{}' as date (expected YYYY-MM-DD)", date_str))?;
            let result = parsed
                .checked_add_signed(chrono::Duration::days(days))
                .ok_or_else(|| "dateadd: date overflow".to_string())?;
            Ok(Value::String(result.format("%Y-%m-%d").to_string()))
        }
        "dateformat" => {
            let date_str = arg(args, 0, name)?.to_string_coerced();
            let pattern = arg(args, 1, name)?.to_string_coerced();
            let parsed = parse_iso_date(&date_str)
                .ok_or_else(|| format!("dateformat: cannot parse '{}' as date (expected YYYY-MM-DD)", date_str))?;
            Ok(Value::String(format_date_pattern(&parsed, &pattern)))
        }
        "datediff" => {
            // datediff(later, earlier) → days between two ISO dates. Positive
            // when `later` is after `earlier`. Use `datediff(today(), duedate)`
            // for "days overdue"; flip the args for "days until due".
            let a_str = arg(args, 0, name)?.to_string_coerced();
            let b_str = arg(args, 1, name)?.to_string_coerced();
            let a = parse_iso_date(&a_str)
                .ok_or_else(|| format!("datediff: cannot parse '{}' as date", a_str))?;
            let b = parse_iso_date(&b_str)
                .ok_or_else(|| format!("datediff: cannot parse '{}' as date", b_str))?;
            let days = (a - b).num_days();
            Ok(Value::Number(days as f64))
        }

        // ── Math functions ──────────────────────────────────────────
        "round" => Ok(Value::Number(arg(args, 0, name)?.to_number()?.round())),
        "floor" => Ok(Value::Number(arg(args, 0, name)?.to_number()?.floor())),
        "ceil"  => Ok(Value::Number(arg(args, 0, name)?.to_number()?.ceil())),
        "abs"   => Ok(Value::Number(arg(args, 0, name)?.to_number()?.abs())),

        // ── Logic ───────────────────────────────────────────────────
        "if" => {
            // if(cond, then_value, else_value) — Text Blaze ternary
            let cond = arg(args, 0, name)?.to_bool();
            if cond {
                Ok(arg(args, 1, name)?.clone())
            } else {
                Ok(arg(args, 2, name)?.clone())
            }
        }

        // ── Random ──────────────────────────────────────────────────
        // random("a", "b", "c") — pick one arg uniformly. Nanos-based; no rand
        // crate. Combine with {set}: `{set greeting = random("hi","hello","hey")}`
        // then `{greeting}` reuses the same picked value later in the expansion.
        "random" => {
            if args.is_empty() {
                return Err("random: needs at least one argument".to_string());
            }
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64 ^ (d.as_secs() as u64))
                .unwrap_or(0);
            let idx = (nanos as usize) % args.len();
            Ok(args[idx].clone())
        }

        _ => Err(format!("unknown function '{}'", name)),
    }
}

fn parse_iso_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

fn format_date_pattern(date: &NaiveDate, pattern: &str) -> String {
    let day = date.day();
    let month = date.month();
    let year = date.year();
    const MONTHS: [&str; 12] = [
        "January", "February", "March", "April", "May", "June",
        "July", "August", "September", "October", "November", "December",
    ];
    let m_idx = month as usize - 1;
    match pattern {
        "DD/MM/YYYY"  => format!("{:02}/{:02}/{:04}", day, month, year),
        "DD/MM/YY"    => format!("{:02}/{:02}/{:02}", day, month, year.rem_euclid(100)),
        "MM/DD/YYYY"  => format!("{:02}/{:02}/{:04}", month, day, year),
        "YYYY-MM-DD"  => format!("{:04}-{:02}-{:02}", year, month, day),
        "D MMMM YYYY" => format!("{} {} {}", day, MONTHS[m_idx], year),
        _             => format!("{:02}/{:02}/{:04}", day, month, year),
    }
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ── Public entry point ─────────────────────────────────────────────────────

/// Evaluate a single expression body (the content between `{=` and `}`).
/// Returns the result as a display string, or `Err(msg)` on parse/eval error.
pub fn evaluate(expr_text: &str, scope: &Scope<'_>) -> Result<String, String> {
    let toks = tokenize(expr_text)?;
    let mut parser = Parser { toks, pos: 0 };
    let ast = parser.parse()?;
    Ok(eval(&ast, scope)?.to_display_string())
}

/// Evaluate an expression body and coerce the result to a boolean — used by
/// `{if expr}` blocks. Errors propagate so the caller can render them inline.
pub fn evaluate_bool(expr_text: &str, scope: &Scope<'_>) -> Result<bool, String> {
    let toks = tokenize(expr_text)?;
    let mut parser = Parser { toks, pos: 0 };
    let ast = parser.parse()?;
    Ok(eval(&ast, scope)?.to_bool())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_scope() -> (HashMap<String, String>, HashMap<String, String>) {
        (HashMap::new(), HashMap::new())
    }

    fn run(expr: &str) -> Result<String, String> {
        let (f, g) = empty_scope();
        let scope = Scope {
            fillin_values: &f,
            global_vars: &g,
            local_vars: HashMap::new(),
            selection: "",
            clipboard: "",
        };
        evaluate(expr, &scope)
    }

    #[test]
    fn arithmetic() {
        assert_eq!(run("2 + 3").unwrap(), "5");
        assert_eq!(run("10 - 4").unwrap(), "6");
        assert_eq!(run("3 * 4").unwrap(), "12");
        assert_eq!(run("10 / 4").unwrap(), "2.5");
        assert_eq!(run("10 % 3").unwrap(), "1");
        assert_eq!(run("-5 + 2").unwrap(), "-3");
        assert_eq!(run("(2 + 3) * 4").unwrap(), "20");
    }

    #[test]
    fn precedence() {
        assert_eq!(run("2 + 3 * 4").unwrap(), "14");
        assert_eq!(run("(2 + 3) * 4").unwrap(), "20");
        assert_eq!(run("10 - 6 / 2").unwrap(), "7");
    }

    #[test]
    fn comparison() {
        assert_eq!(run("2 == 2").unwrap(), "yes");
        assert_eq!(run("2 != 3").unwrap(), "yes");
        assert_eq!(run("5 > 3").unwrap(), "yes");
        assert_eq!(run("5 < 3").unwrap(), "no");
        assert_eq!(run("5 >= 5").unwrap(), "yes");
        assert_eq!(run("\"a\" == \"a\"").unwrap(), "yes");
    }

    #[test]
    fn logical() {
        assert_eq!(run("yes && yes").unwrap(), "yes");
        assert_eq!(run("yes && no").unwrap(), "no");
        assert_eq!(run("no || yes").unwrap(), "yes");
        assert_eq!(run("!yes").unwrap(), "no");
    }

    #[test]
    fn string_concat() {
        assert_eq!(run("\"hi \" & \"there\"").unwrap(), "hi there");
        assert_eq!(run("\"count: \" & 42").unwrap(), "count: 42");
    }

    #[test]
    fn string_functions() {
        assert_eq!(run("upper(\"hi\")").unwrap(), "HI");
        assert_eq!(run("lower(\"HI\")").unwrap(), "hi");
        assert_eq!(run("trim(\"  x  \")").unwrap(), "x");
        assert_eq!(run("len(\"abc\")").unwrap(), "3");
        assert_eq!(run("substring(\"abcdef\", 2, 3)").unwrap(), "bcd");
        assert_eq!(run("replace(\"good job\", \"good\", \"great\")").unwrap(), "great job");
        assert_eq!(run("contains(\"hello world\", \"world\")").unwrap(), "yes");
        assert_eq!(run("startswith(\"hello\", \"he\")").unwrap(), "yes");
        assert_eq!(run("endswith(\"hello\", \"lo\")").unwrap(), "yes");
    }

    #[test]
    fn math_functions() {
        assert_eq!(run("round(1.7)").unwrap(), "2");
        assert_eq!(run("floor(1.7)").unwrap(), "1");
        assert_eq!(run("ceil(1.2)").unwrap(), "2");
        assert_eq!(run("abs(-3)").unwrap(), "3");
    }

    #[test]
    fn if_ternary() {
        assert_eq!(run("if(yes, \"a\", \"b\")").unwrap(), "a");
        assert_eq!(run("if(no, \"a\", \"b\")").unwrap(), "b");
        assert_eq!(run("if(2 > 1, \"big\", \"small\")").unwrap(), "big");
    }

    #[test]
    fn scope_lookup() {
        let mut f = HashMap::new();
        f.insert("name".into(), "John".into());
        let g = HashMap::new();
        let scope = Scope {
            fillin_values: &f,
            global_vars: &g,
            local_vars: HashMap::new(),
            selection: "",
            clipboard: "",
        };
        assert_eq!(evaluate("upper(name)", &scope).unwrap(), "JOHN");
        assert_eq!(evaluate("\"hi \" & name", &scope).unwrap(), "hi John");
    }

    #[test]
    fn errors() {
        assert!(run("2 +").is_err());
        assert!(run("unknown_fn(1)").is_err());
        assert!(run("\"unterminated").is_err());
        assert!(run("10 / 0").is_err());
    }
}
