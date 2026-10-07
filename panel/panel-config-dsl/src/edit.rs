//! Model edits applied to the files they were read from: unchanged resources
//! keep their text and comments, changed ones are reprinted in place, removed
//! ones disappear with their comments and new ones join the `http` block.

use crate::{print, Insertion, Lowered, Sources, ENTRY, LANGUAGE_VERSION};
use panel_config_model::ConfigModel;
use panel_dsl::{format_directive, Directive, Span};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

/// Every resource that has a block of its own, by resource path.
pub(crate) fn blocks(model: &ConfigModel) -> Vec<(String, Directive)> {
    let mut blocks = Vec::new();
    blocks.extend(model.tls_profiles.iter().map(|profile| {
        (
            format!("tls-profiles/{}", profile.id),
            print::tls_profile(profile),
        )
    }));
    blocks.extend(model.security_policies.iter().map(|policy| {
        (
            format!("security-policies/{}", policy.id),
            print::security_policy(policy),
        )
    }));
    blocks.extend(model.http_policies.iter().map(|policy| {
        (
            format!("http-policies/{}", policy.id),
            print::http_policy(policy),
        )
    }));
    blocks.extend(model.cache_policies.iter().map(|policy| {
        (
            format!("cache-policies/{}", policy.id),
            print::cache_policy(policy),
        )
    }));
    blocks.extend(print::cache_store(&model.cache).map(|store| ("cache".to_owned(), store)));
    blocks.extend(model.listeners.iter().map(|listener| {
        (
            format!("listeners/{}", listener.id),
            print::listener(listener, model),
        )
    }));
    blocks.extend(model.upstreams.iter().map(|upstream| {
        (
            format!("upstreams/{}", upstream.id),
            print::upstream(upstream),
        )
    }));
    blocks.extend(
        model
            .sites
            .iter()
            .filter(|site| !site.is_deleted())
            .map(|site| (format!("sites/{}", site.id), print::server(site, model))),
    );
    blocks
}

/// A block printed at `depth` without the indentation of its first line or
/// its final line break, to replace a block that starts at its name.
fn in_place(directive: &Directive, depth: usize) -> String {
    let printed = format_directive(directive, depth);
    printed
        .trim_start_matches(' ')
        .trim_end_matches('\n')
        .to_owned()
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |newline| newline + 1)
}

/// The lines `span` occupies, with their line break, when nothing else is on
/// them; a blank line left doubled by the removal goes as well.
fn removal(text: &str, span: Span) -> Range<usize> {
    let mut start = span.start;
    let before = line_start(text, start);
    if text[before..start].trim().is_empty() {
        start = before;
    }
    let mut end = span.end;
    let rest = &text[end..];
    let line_end = rest
        .find('\n')
        .map_or(text.len(), |newline| end + newline + 1);
    if text[end..line_end].trim().is_empty() {
        end = line_end;
        let blank_before = start >= 2 && text[..start].ends_with("\n\n");
        let next_end = text[end..]
            .find('\n')
            .map_or(text.len(), |newline| end + newline + 1);
        if blank_before && end < text.len() && text[end..next_end].trim().is_empty() {
            end = next_end;
        }
    }
    start..end
}

fn apply(text: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0.start));
    let mut out = text.to_owned();
    for (range, replacement) in edits {
        out.replace_range(range, &replacement);
    }
    out
}

/// The files `lowered` was read from, rewritten to describe `next`.
pub fn reconcile(sources: &Sources, lowered: &Lowered, next: &ConfigModel) -> Sources {
    let before: BTreeMap<String, String> = blocks(&lowered.model)
        .into_iter()
        .map(|(key, directive)| (key, format_directive(&directive, 0)))
        .collect();
    let after = blocks(next);
    let kept: BTreeSet<&str> = after.iter().map(|(key, _)| key.as_str()).collect();
    let mut edits: BTreeMap<String, Vec<(Range<usize>, String)>> = BTreeMap::new();
    for key in before.keys().filter(|key| !kept.contains(key.as_str())) {
        if let Some(origin) = lowered.origins.get(key) {
            let text = sources.get(&origin.file).unwrap_or_default();
            edits
                .entry(origin.file.clone())
                .or_default()
                .push((removal(text, origin.outer), String::new()));
        }
    }
    let mut appended = Vec::new();
    for (key, directive) in &after {
        match (before.get(key), lowered.origins.get(key)) {
            (Some(old), Some(origin)) => {
                if *old != format_directive(directive, 0) {
                    edits
                        .entry(origin.file.clone())
                        .or_default()
                        .push((origin.span.range(), in_place(directive, origin.depth)));
                }
            }
            _ => appended.push(directive),
        }
    }
    let mut files = sources.clone();
    for (file, file_edits) in edits {
        let text = sources.get(&file).unwrap_or_default();
        files.insert(file, apply(text, file_edits));
    }
    if !appended.is_empty() {
        let main = files.get(ENTRY).unwrap_or_default().to_owned();
        files.insert(ENTRY, append(&main, &appended));
    }
    for (path, _) in sources
        .files()
        .filter(|(path, _)| crate::source::is_lua(path))
    {
        if !next.lua.files.contains_key(path) {
            files.remove(path);
        }
    }
    for (path, text) in &next.lua.files {
        if files.get(path) != Some(text.as_str()) {
            files.insert(path.clone(), text.clone());
        }
    }
    files
}

/// `blocks` added at the end of the `http` block, which is created when the
/// file has none.
fn append(text: &str, blocks: &[&Directive]) -> String {
    let parsed = panel_dsl::parse(ENTRY, text);
    let close = parsed
        .document
        .directives
        .iter()
        .find(|directive| directive.name.value == "http")
        .and_then(|http| http.block())
        .and_then(|block| block.close);
    let mut added = String::new();
    for block in blocks {
        added.push('\n');
        added.push_str(&format_directive(block, 1));
    }
    match close {
        Some(close) => {
            let at = line_start(text, close.start);
            let mut out = text[..at].to_owned();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if out.ends_with("{\n") {
                added.remove(0);
            }
            out.push_str(&added);
            out.push_str(&text[at..]);
            out
        }
        None => {
            let mut out = text.to_owned();
            if out.trim().is_empty() {
                out = format!("language_version {LANGUAGE_VERSION};\n");
            } else if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("\nhttp {");
            out.push_str(&added);
            out.push_str("}\n");
            out
        }
    }
}

/// Each file formatted canonically; files with syntax errors stay as they
/// are, and their errors are returned.
pub fn format_files(sources: &Sources) -> (Sources, Vec<panel_errors::Diagnostic>) {
    let mut formatted = sources.clone();
    let mut diagnostics = Vec::new();
    for (path, text) in sources
        .files()
        .filter(|(path, _)| !crate::source::is_lua(path))
    {
        let parsed = panel_dsl::parse(path, text);
        if parsed.is_valid() {
            formatted.insert(path, panel_dsl::format(&parsed.document));
        } else {
            diagnostics.extend(parsed.diagnostics);
        }
    }
    (formatted, diagnostics)
}

/// The files with identifiers assigned while reading them written in.
pub fn write_identifiers(sources: &Sources, insertions: &[Insertion]) -> Sources {
    let mut by_file: BTreeMap<&str, Vec<(Range<usize>, String)>> = BTreeMap::new();
    for insertion in insertions {
        by_file
            .entry(&insertion.file)
            .or_default()
            .push((insertion.offset..insertion.offset, insertion.text.clone()));
    }
    let mut files = sources.clone();
    for (file, edits) in by_file {
        let text = sources.get(file).unwrap_or_default();
        files.insert(file, apply(text, edits));
    }
    files
}
