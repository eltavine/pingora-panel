//! What checking finds in a script without running it (ADR 0039): modules
//! `require` cannot load, `ngx` functions this gateway does not provide or
//! the handler's phase does not allow, and globals the script writes.
//!
//! Scripts are read token by token with Lua's lexical rules, tracking the
//! locals of each block as luacheck does; what a script computes at run
//! time, such as a module name it builds, escapes the check.

use crate::{
    api::{contexts::context, Api, BUILT_IN_MODULES, UNAVAILABLE},
    exchange::Phase,
    program::Source,
};
use std::collections::BTreeSet;

/// What a finding is about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FindingKind {
    /// `require` names a module that is neither built in nor a file of the
    /// configuration.
    UnknownModule,
    /// An `ngx` function lua-nginx-module has and this gateway does not.
    Unavailable,
    /// An `ngx` function the handler's phase does not allow.
    NotInPhase,
    /// An assignment to a global variable, which stays with the request or
    /// the module rather than being shared.
    GlobalWrite,
}

/// A problem at a line of the file the script comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding {
    pub kind: FindingKind,
    pub line: u32,
    pub message: String,
}

/// How a script runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// A handler of the phase; `init` and `init_worker` set the globals
    /// requests read, so their writes are not findings.
    Handler(Phase),
    /// A module `require` loads, in an environment of its own.
    Module,
}

/// What checking one script found, and the modules it loads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lint {
    pub findings: Vec<Finding>,
    /// The modules it names in `require`.
    pub requires: BTreeSet<String>,
    /// Whether it calls `require` with a name computed at run time.
    pub dynamic_require: bool,
}

/// Checks `source` as `role`. `module` tells whether the configuration has
/// a module of that name; built-in modules are known here.
pub fn lint(source: &Source, role: Role, module: &dyn Fn(&str) -> bool) -> Lint {
    let tokens = tokens(&source.text);
    let mut checker = Checker {
        tokens: &tokens,
        role,
        offset: source.line.saturating_sub(1),
        blocks: vec![Block::new(0, Vec::new())],
        depth: 0,
        pending: Vec::new(),
        lint: Lint::default(),
    };
    checker.run(module);
    checker.lint
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Name,
    String,
    Number,
    Symbol,
}

#[derive(Clone, Debug)]
struct Token {
    kind: Kind,
    text: String,
    line: u32,
}

const KEYWORDS: [&str; 22] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

const SYMBOLS: [&str; 18] = [
    "...", "//=", "..=", "==", "~=", "<=", ">=", "..", "::", "+=", "-=", "*=", "/=", "%=", "^=",
    "//", "->", "=",
];

fn is_assignment(token: Option<&Token>) -> bool {
    token.is_some_and(|token| {
        token.kind == Kind::Symbol
            && matches!(
                token.text.as_str(),
                "=" | "+=" | "-=" | "*=" | "/=" | "//=" | "%=" | "^=" | "..="
            )
    })
}

/// The level of the long bracket opening at `at`, as in `[==[`.
fn long_bracket(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'[') {
        return None;
    }
    let level = bytes[at + 1..]
        .iter()
        .take_while(|&&byte| byte == b'=')
        .count();
    (bytes.get(at + 1 + level) == Some(&b'[')).then_some(level)
}

/// Where the long bracket of `level` opened at `at` ends, and its content.
fn long_end(text: &str, at: usize, level: usize) -> (usize, &str) {
    let bytes = text.as_bytes();
    let start = at + level + 2;
    let mut index = start;
    while index < bytes.len() {
        if bytes[index] == b']'
            && bytes[index + 1..]
                .iter()
                .take(level)
                .all(|&byte| byte == b'=')
            && bytes.get(index + 1 + level) == Some(&b']')
        {
            return (index + level + 2, &text[start..index]);
        }
        index += 1;
    }
    (bytes.len(), &text[start..])
}

fn tokens(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    let mut line = 1u32;
    let lines_in = |part: &str| part.bytes().filter(|&byte| byte == b'\n').count() as u32;
    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b'\n' => {
                line += 1;
                index += 1;
            }
            _ if byte.is_ascii_whitespace() => index += 1,
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index += 2;
                match long_bracket(bytes, index) {
                    Some(level) => {
                        let (end, _) = long_end(text, index, level);
                        line += lines_in(&text[index..end]);
                        index = end;
                    }
                    None => {
                        while index < bytes.len() && bytes[index] != b'\n' {
                            index += 1;
                        }
                    }
                }
            }
            b'[' if long_bracket(bytes, index).is_some() => {
                let level = long_bracket(bytes, index).unwrap_or(0);
                let (end, content) = long_end(text, index, level);
                out.push(Token {
                    kind: Kind::String,
                    text: content.to_owned(),
                    line,
                });
                line += lines_in(&text[index..end]);
                index = end;
            }
            b'"' | b'\'' | b'`' => {
                let start = index + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != byte {
                    if bytes[end] == b'\\' {
                        end += 1;
                    } else if bytes[end] == b'\n' && byte != b'`' {
                        break;
                    }
                    end += 1;
                }
                let end = end.min(bytes.len());
                out.push(Token {
                    kind: Kind::String,
                    text: text[start..end].to_owned(),
                    line,
                });
                line += lines_in(&text[start..end]);
                let closed = bytes.get(end) == Some(&byte);
                index = if closed { end + 1 } else { end };
            }
            _ if byte.is_ascii_digit()
                || (byte == b'.' && bytes.get(index + 1).is_some_and(u8::is_ascii_digit)) =>
            {
                let start = index;
                while index < bytes.len() {
                    let next = bytes[index];
                    let exponent = index > start
                        && matches!(bytes[index - 1], b'e' | b'E' | b'p' | b'P')
                        && matches!(next, b'+' | b'-');
                    if next.is_ascii_alphanumeric() || next == b'_' || next == b'.' || exponent {
                        index += 1;
                    } else {
                        break;
                    }
                }
                out.push(Token {
                    kind: Kind::Number,
                    text: text[start..index].to_owned(),
                    line,
                });
            }
            _ if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                out.push(Token {
                    kind: Kind::Name,
                    text: text[start..index].to_owned(),
                    line,
                });
            }
            _ => {
                let rest = &text[index..];
                let symbol = SYMBOLS
                    .iter()
                    .find(|symbol| rest.starts_with(**symbol))
                    .map_or_else(
                        || rest.chars().next().map_or(1, char::len_utf8),
                        |symbol| symbol.len(),
                    );
                out.push(Token {
                    kind: Kind::Symbol,
                    text: rest[..symbol].to_owned(),
                    line,
                });
                index += symbol;
            }
        }
    }
    out
}

struct Block {
    /// The bracket depth statements of the block are written at.
    base: usize,
    locals: BTreeSet<String>,
}

impl Block {
    fn new(base: usize, locals: Vec<String>) -> Self {
        Self {
            base,
            locals: locals.into_iter().collect(),
        }
    }
}

struct Checker<'t> {
    tokens: &'t [Token],
    role: Role,
    offset: u32,
    blocks: Vec<Block>,
    depth: usize,
    /// Loop variables, declared when the loop's block opens.
    pending: Vec<String>,
    lint: Lint,
}

impl Checker<'_> {
    fn at(&self, index: usize) -> Option<&Token> {
        self.tokens.get(index)
    }

    fn is(&self, index: usize, kind: Kind, text: &str) -> bool {
        self.at(index)
            .is_some_and(|token| token.kind == kind && token.text == text)
    }

    fn is_name(&self, index: usize) -> bool {
        self.at(index).is_some_and(|token| {
            token.kind == Kind::Name && !KEYWORDS.contains(&token.text.as_str())
        })
    }

    fn declared(&self, name: &str) -> bool {
        self.blocks.iter().any(|block| block.locals.contains(name))
    }

    fn declare(&mut self, name: &str) {
        if let Some(block) = self.blocks.last_mut() {
            block.locals.insert(name.to_owned());
        }
    }

    fn open(&mut self, locals: Vec<String>) {
        self.blocks.push(Block::new(self.depth, locals));
    }

    fn close(&mut self) {
        if self.blocks.len() > 1 {
            self.blocks.pop();
        }
    }

    fn find(&mut self, kind: FindingKind, line: u32, message: String) {
        self.lint.findings.push(Finding {
            kind,
            line: line + self.offset,
            message,
        });
    }

    fn global_write(&mut self, name: &str, line: u32) {
        if self.declared(name) {
            return;
        }
        let message = match self.role {
            Role::Handler(Phase::Init | Phase::InitWorker) => return,
            Role::Handler(_) => format!(
                "the script writes the global {name}, which stays with this request; declare it local"
            ),
            Role::Module => format!(
                "the module writes the global {name}, which stays in the module's own environment; declare it local or return it"
            ),
        };
        self.find(FindingKind::GlobalWrite, line, message);
    }

    /// Skips a Luau type annotation after `:` at `index`.
    fn skip_type(&self, mut index: usize) -> usize {
        if !self.is(index, Kind::Symbol, ":") {
            return index;
        }
        index += 1;
        while self.is_name(index) {
            index += 1;
            if self.is(index, Kind::Symbol, ".") {
                index += 1;
            } else {
                break;
            }
        }
        if self.is(index, Kind::Symbol, "?") {
            index += 1;
        }
        index
    }

    fn run(&mut self, module: &dyn Fn(&str) -> bool) {
        let mut index = 0;
        while let Some(token) = self.at(index).cloned() {
            let after_field = index
                .checked_sub(1)
                .and_then(|before| self.at(before))
                .is_some_and(|before| {
                    before.kind == Kind::Symbol && matches!(before.text.as_str(), "." | ":" | "::")
                });
            match (token.kind, token.text.as_str()) {
                (Kind::Symbol, "(" | "{" | "[") => self.depth += 1,
                (Kind::Symbol, ")" | "}" | "]") => self.depth = self.depth.saturating_sub(1),
                (Kind::Name, "local") => {
                    index = self.local(index + 1);
                    continue;
                }
                (Kind::Name, "function") => {
                    index = self.function(index + 1);
                    continue;
                }
                (Kind::Name, "for") => {
                    let mut next = index + 1;
                    while self.is_name(next) {
                        self.pending.push(self.tokens[next].text.clone());
                        next = self.skip_type(next + 1);
                        if self.is(next, Kind::Symbol, ",") {
                            next += 1;
                        } else {
                            break;
                        }
                    }
                    index = next;
                    continue;
                }
                (Kind::Name, "do") => {
                    let locals = std::mem::take(&mut self.pending);
                    self.open(locals);
                }
                (Kind::Name, "then" | "repeat") => self.open(Vec::new()),
                (Kind::Name, "else") => {
                    self.close();
                    self.open(Vec::new());
                }
                (Kind::Name, "elseif" | "end" | "until") => self.close(),
                (Kind::Name, "require") if !after_field => self.require(index, module),
                (Kind::Name, "ngx") if !after_field => {
                    index = self.ngx(index);
                    continue;
                }
                (Kind::Name, name)
                    if !after_field
                        && !KEYWORDS.contains(&name)
                        && self
                            .blocks
                            .last()
                            .is_some_and(|block| block.base == self.depth) =>
                {
                    index = self.assignment(index);
                    continue;
                }
                _ => {}
            }
            index += 1;
        }
    }

    /// `local name, ...` or `local function name`; returns where to go on.
    fn local(&mut self, mut index: usize) -> usize {
        if self.is(index, Kind::Name, "function") {
            if let Some(name) = self.at(index + 1).filter(|_| self.is_name(index + 1)) {
                let name = name.text.clone();
                self.declare(&name);
            }
            return index;
        }
        while self.is_name(index) {
            let name = self.tokens[index].text.clone();
            self.declare(&name);
            index = self.skip_type(index + 1);
            if self.is(index, Kind::Symbol, "<") {
                while self.at(index).is_some() && !self.is(index, Kind::Symbol, ">") {
                    index += 1;
                }
                index += 1;
            }
            if self.is(index, Kind::Symbol, ",") {
                index += 1;
            } else {
                break;
            }
        }
        index
    }

    /// `function [name[.field][:method]] (params) body end`, from after the
    /// keyword; returns the first token of the body.
    fn function(&mut self, mut index: usize) -> usize {
        let mut locals = Vec::new();
        if self.is_name(index) {
            let name = self.tokens[index].clone();
            let mut plain = true;
            index += 1;
            while self.at(index).is_some_and(|token| {
                token.kind == Kind::Symbol && matches!(token.text.as_str(), "." | ":")
            }) {
                if self.is(index, Kind::Symbol, ":") {
                    locals.push("self".to_owned());
                }
                plain = false;
                index += 2;
            }
            if plain {
                self.global_write(&name.text, name.line);
            }
        }
        if self.is(index, Kind::Symbol, "<") {
            while self.at(index).is_some() && !self.is(index, Kind::Symbol, ">") {
                index += 1;
            }
            index += 1;
        }
        if self.is(index, Kind::Symbol, "(") {
            index += 1;
            let mut nesting = 0usize;
            while let Some(token) = self.at(index) {
                match (token.kind, token.text.as_str()) {
                    (Kind::Symbol, "(" | "{" | "[") => nesting += 1,
                    (Kind::Symbol, ")") if nesting == 0 => break,
                    (Kind::Symbol, ")" | "}" | "]") => nesting -= 1,
                    (Kind::Name, _) if nesting == 0 && self.is_name(index) => {
                        let after = self.at(index.saturating_sub(1));
                        if !after.is_some_and(|before| before.text == ":") {
                            locals.push(token.text.clone());
                        }
                    }
                    _ => {}
                }
                index += 1;
            }
            index += 1;
        }
        self.open(locals);
        index
    }

    /// `require "name"`, `require("name")` or `require [[name]]`.
    fn require(&mut self, index: usize, module: &dyn Fn(&str) -> bool) {
        let named = match self.at(index + 1) {
            Some(token) if token.kind == Kind::String => Some(token.clone()),
            Some(token) if token.kind == Kind::Symbol && token.text == "(" => self
                .at(index + 2)
                .filter(|argument| {
                    argument.kind == Kind::String && self.is(index + 3, Kind::Symbol, ")")
                })
                .cloned(),
            _ => None,
        };
        let Some(name) = named else {
            if self.is(index + 1, Kind::Symbol, "(") {
                self.lint.dynamic_require = true;
            }
            return;
        };
        self.lint.requires.insert(name.text.clone());
        if !BUILT_IN_MODULES.contains(&name.text.as_str()) && !module(&name.text) {
            self.find(
                FindingKind::UnknownModule,
                name.line,
                format!(
                    "module {:?} is neither built in nor a file under lua/ of the configuration",
                    name.text
                ),
            );
        }
    }

    /// `ngx.a.b`: whether this gateway has it, and whether the phase allows it.
    fn ngx(&mut self, index: usize) -> usize {
        let line = self.tokens[index].line;
        let mut path = "ngx".to_owned();
        let mut next = index + 1;
        while self.is(next, Kind::Symbol, ".")
            && self
                .at(next + 1)
                .is_some_and(|token| token.kind == Kind::Name)
        {
            path.push('.');
            path.push_str(&self.tokens[next + 1].text);
            next += 2;
        }
        if UNAVAILABLE.contains(&path.as_str()) {
            self.find(
                FindingKind::Unavailable,
                line,
                format!("{path} is not available in Pingora Panel"),
            );
        } else if let (Role::Handler(phase), Some(api)) = (self.role, Api::by_path(&path)) {
            if !api.allows(phase) {
                self.find(
                    FindingKind::NotInPhase,
                    line,
                    format!("{path} is disabled in {}", context(phase)),
                );
            }
        }
        next
    }

    /// `name = ...`, `name, other = ...` or `name += ...` at the start of a
    /// statement.
    fn assignment(&mut self, index: usize) -> usize {
        let before = index.checked_sub(1).and_then(|before| self.at(before));
        if before.is_some_and(|before| {
            before.kind == Kind::Name && matches!(before.text.as_str(), "type" | "export")
        }) {
            return index + 1;
        }
        let mut targets = vec![self.tokens[index].clone()];
        let mut next = index + 1;
        while self.is(next, Kind::Symbol, ",") && self.is_name(next + 1) {
            targets.push(self.tokens[next + 1].clone());
            next += 2;
        }
        if is_assignment(self.at(next)) {
            for target in targets {
                self.global_write(&target.text, target.line);
            }
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str, role: Role) -> Vec<(FindingKind, u32, String)> {
        lint(&Source::new("x.lua", text, 1), role, &|name| {
            name == "local.helper"
        })
        .findings
        .into_iter()
        .map(|finding| (finding.kind, finding.line, finding.message))
        .collect()
    }

    #[test]
    fn globals_written_outside_locals_are_found() {
        let text = r#"
local cache, count = {}, 0
local function helper(a, b) total = a + b; return a end
function api(x) local y = x; y = 2; return y end
for i, v in ipairs(cache) do i = v end
count = count + 1
local t = { field = 1, nested = { deep = 2 } }
t.field = 3
t["k"] = 4
leaked, count = 1, 2
if count then
    local inside = 1
    inside = 2
    outside = 3
else
    other += 1
end
call(function(z) z = 1; inner = 2 end)
local obj = {}
function obj:method() self.value = 1 end
-- comment = 1
local s = "quoted = 1" .. [[long = 2]]
"#;
        let globals: Vec<_> = found(text, Role::Module)
            .into_iter()
            .filter(|(kind, _, _)| *kind == FindingKind::GlobalWrite)
            .map(|(_, line, message)| (line, message.split_whitespace().nth(5).unwrap().to_owned()))
            .collect();
        assert_eq!(
            globals,
            [
                (3, "total,".to_owned()),
                (4, "api,".to_owned()),
                (10, "leaked,".to_owned()),
                (14, "outside,".to_owned()),
                (16, "other,".to_owned()),
                (18, "inner,".to_owned()),
            ]
        );
        assert!(found("LIMITS = { per_minute = 60 }", Role::Handler(Phase::Init)).is_empty());
        let handler = found("seen = true", Role::Handler(Phase::Access));
        assert!(
            handler[0].2.contains("stays with this request"),
            "{handler:?}"
        );
    }

    #[test]
    fn requires_are_checked_against_the_modules_there_are() {
        let result = lint(
            &Source::new("main.conf", "local a = require \"cjson\"\nlocal b = require(\"local.helper\")\nlocal c = require('resty.redis')\nlocal d = require(name)\n", 10),
            Role::Handler(Phase::Access),
            &|name| name == "local.helper",
        );
        assert_eq!(
            result.requires.into_iter().collect::<Vec<_>>(),
            ["cjson", "local.helper", "resty.redis"]
        );
        assert!(result.dynamic_require);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].kind, FindingKind::UnknownModule);
        assert_eq!(result.findings[0].line, 12);
        assert!(result.findings[0].message.contains("\"resty.redis\""));
    }

    #[test]
    fn ngx_functions_are_checked_against_the_gateway_and_the_phase() {
        let text = "ngx.say(ngx.var.host)\nlocal r = ngx.location.capture('/x')\nngx.header['X'] = '1'\nlocal ok = ngx.req.get_headers()\n-- ngx.say in a comment\nlocal s = 'ngx.exit(1)'\n";
        let in_header_filter = found(text, Role::Handler(Phase::HeaderFilter));
        assert_eq!(
            in_header_filter,
            [
                (
                    FindingKind::NotInPhase,
                    1,
                    "ngx.say is disabled in header_filter_by_lua*".to_owned()
                ),
                (
                    FindingKind::Unavailable,
                    2,
                    "ngx.location.capture is not available in Pingora Panel".to_owned()
                ),
            ]
        );
        let in_access = found(text, Role::Handler(Phase::Access));
        assert_eq!(in_access.len(), 1);
        assert_eq!(
            found("ngx.print('x')", Role::Handler(Phase::Log))[0].2,
            "ngx.print is disabled in log_by_lua*"
        );
        assert!(found("ngx.req.set_uri('/', true)", Role::Module).is_empty());
    }
}
