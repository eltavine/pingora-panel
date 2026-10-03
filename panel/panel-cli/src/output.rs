//! Renders API values as tables for people or JSON for scripts.

use clap::ValueEnum;
use serde_json::Value;
use std::io::Write;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum Format {
    #[default]
    Table,
    Json,
}

/// A column title and how to read it from one item.
pub type Column = (&'static str, fn(&Value) -> String);

pub struct Output {
    pub format: Format,
    pub quiet: bool,
}

pub fn text(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(text).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

impl Output {
    /// A list of items as rows.
    pub fn list(&self, items: &Value, columns: &[Column]) {
        if self.quiet {
            return;
        }
        match self.format {
            Format::Json => self.json(items),
            Format::Table => {
                let rows: Vec<Vec<String>> = items
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| columns.iter().map(|(_, read)| read(item)).collect())
                            .collect()
                    })
                    .unwrap_or_default();
                let mut widths: Vec<usize> = columns.iter().map(|(title, _)| title.len()).collect();
                for row in &rows {
                    for (width, cell) in widths.iter_mut().zip(row) {
                        *width = (*width).max(cell.chars().count());
                    }
                }
                let mut out = std::io::stdout().lock();
                let line = |cells: Vec<String>| {
                    cells
                        .iter()
                        .zip(&widths)
                        .map(|(cell, width)| format!("{cell:<width$}"))
                        .collect::<Vec<_>>()
                        .join("  ")
                        .trim_end()
                        .to_owned()
                };
                let _ = writeln!(
                    out,
                    "{}",
                    line(
                        columns
                            .iter()
                            .map(|(title, _)| (*title).to_owned())
                            .collect()
                    )
                );
                for row in rows {
                    let _ = writeln!(out, "{}", line(row));
                }
            }
        }
    }

    /// One item as `field: value` lines.
    pub fn item(&self, item: &Value, fields: &[Column]) {
        if self.quiet {
            return;
        }
        match self.format {
            Format::Json => self.json(item),
            Format::Table => {
                let width = fields
                    .iter()
                    .map(|(title, _)| title.len())
                    .max()
                    .unwrap_or(0);
                let mut out = std::io::stdout().lock();
                for (title, read) in fields {
                    let _ = writeln!(out, "{title:<width$}  {}", read(item));
                }
            }
        }
    }

    pub fn json(&self, value: &Value) {
        if !self.quiet {
            println!(
                "{}",
                serde_json::to_string_pretty(value).expect("JSON values serialize")
            );
        }
    }

    /// A confirmation for people; scripts read the exit code.
    pub fn done(&self, message: &str, value: &Value) {
        if self.quiet {
            return;
        }
        match self.format {
            Format::Json => self.json(value),
            Format::Table => println!("{message}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_render_as_compact_cells() {
        assert_eq!(text(&Value::Null), "-");
        assert_eq!(text(&serde_json::json!(["a", "b"])), "a,b");
        assert_eq!(text(&serde_json::json!(3)), "3");
        assert_eq!(text(&serde_json::json!("x")), "x");
    }
}
