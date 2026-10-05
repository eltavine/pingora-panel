//! Scripts compiled once for a configuration: phase handlers, the modules
//! `require` loads, `init_by_lua` and `init_worker_by_lua`, and the shared
//! dictionaries they use.

use crate::exchange::Limits;
use mlua::chunk::Compiler;
use std::{collections::BTreeMap, fmt};

/// Lua source text and where it sits in the configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Source {
    /// The file it comes from, such as `main.conf` or `lua/auth.lua`.
    pub name: String,
    pub text: String,
    /// The line of `name` the text starts on, from 1.
    pub line: u32,
}

impl Source {
    pub fn new(name: impl Into<String>, text: impl Into<String>, line: u32) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
            line: line.max(1),
        }
    }
}

/// A problem in a script, located in the file it comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub source: String,
    pub line: Option<u32>,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "{}:{line}: {}", self.source, self.message),
            None => write!(formatter, "{}: {}", self.source, self.message),
        }
    }
}

pub(crate) fn compiler() -> Compiler {
    Compiler::new()
        .set_optimization_level(2)
        .set_debug_level(1)
        .set_type_info_level(1)
}

/// Compiles `source` to Luau bytecode. Lines in errors are lines of the file
/// the source comes from.
pub fn compile(source: &Source) -> Result<Vec<u8>, Diagnostic> {
    let mut text = "\n".repeat(source.line as usize - 1);
    text.push_str(&source.text);
    compiler()
        .compile(text)
        .map_err(|error| syntax_diagnostic(&source.name, error))
}

fn syntax_diagnostic(name: &str, error: mlua::Error) -> Diagnostic {
    let text = match error {
        mlua::Error::SyntaxError { message, .. } => message,
        other => other.to_string(),
    };
    let located = text
        .split_once(": ")
        .and_then(|(line, message)| Some((line.parse::<u32>().ok()?, message.to_owned())));
    match located {
        Some((line, message)) => Diagnostic {
            source: name.to_owned(),
            line: Some(line),
            message,
        },
        None => Diagnostic {
            source: name.to_owned(),
            line: None,
            message: text,
        },
    }
}

/// A handler of the program, valid in the runtime built from it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HandlerId(pub(crate) u32);

#[derive(Clone, Debug)]
pub(crate) struct Compiled {
    pub name: String,
    pub bytecode: Vec<u8>,
}

/// A shared dictionary every VM of the runtime sees.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SharedDict {
    pub name: String,
    /// Bytes its keys and values may take.
    pub capacity: usize,
}

/// Everything a configuration's scripts need, compiled.
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub(crate) handlers: Vec<Compiled>,
    pub(crate) modules: BTreeMap<String, Compiled>,
    pub(crate) init: Option<HandlerId>,
    pub(crate) init_worker: Option<HandlerId>,
    pub(crate) init_limits: Limits,
    pub(crate) dicts: Vec<SharedDict>,
}

impl Program {
    pub fn builder() -> ProgramBuilder {
        ProgramBuilder::default()
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty() && self.modules.is_empty()
    }

    pub fn shared_dicts(&self) -> &[SharedDict] {
        &self.dicts
    }

    pub fn modules(&self) -> impl Iterator<Item = &str> {
        self.modules.keys().map(String::as_str)
    }
}

/// Collects and compiles a program's scripts, keeping every diagnostic.
#[derive(Debug, Default)]
pub struct ProgramBuilder {
    program: Program,
    diagnostics: Vec<Diagnostic>,
}

impl ProgramBuilder {
    /// Adds a phase handler; on a syntax error the diagnostic is kept and
    /// the handler raises it when it runs.
    pub fn handler(&mut self, source: &Source) -> HandlerId {
        let id = HandlerId(self.program.handlers.len() as u32);
        let bytecode = self.compiled(source);
        self.program.handlers.push(Compiled {
            name: source.name.clone(),
            bytecode,
        });
        id
    }

    /// Adds the module `require(name)` loads.
    pub fn module(&mut self, name: impl Into<String>, source: &Source) -> &mut Self {
        let bytecode = self.compiled(source);
        self.program.modules.insert(
            name.into(),
            Compiled {
                name: source.name.clone(),
                bytecode,
            },
        );
        self
    }

    pub fn init(&mut self, handler: HandlerId) -> &mut Self {
        self.program.init = Some(handler);
        self
    }

    pub fn init_worker(&mut self, handler: HandlerId) -> &mut Self {
        self.program.init_worker = Some(handler);
        self
    }

    /// Limits for `init_by_lua` and `init_worker_by_lua`.
    pub fn init_limits(&mut self, limits: Limits) -> &mut Self {
        self.program.init_limits = limits;
        self
    }

    pub fn shared_dict(&mut self, name: impl Into<String>, capacity: usize) -> &mut Self {
        self.program.dicts.push(SharedDict {
            name: name.into(),
            capacity,
        });
        self
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// The program, or every diagnostic its scripts have.
    pub fn build(self) -> Result<Program, Vec<Diagnostic>> {
        if self.diagnostics.is_empty() {
            Ok(self.program)
        } else {
            Err(self.diagnostics)
        }
    }

    fn compiled(&mut self, source: &Source) -> Vec<u8> {
        match compile(source) {
            Ok(bytecode) => bytecode,
            Err(diagnostic) => {
                self.diagnostics.push(diagnostic);
                Vec::new()
            }
        }
    }
}
