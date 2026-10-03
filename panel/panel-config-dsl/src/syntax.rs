//! The syntax tree of a file: each directive as written, for inspecting how
//! the language reads a configuration.

use crate::Sources;
use panel_dsl::{Directive, LineIndex, Trivia};
use panel_errors::Diagnostic;
use serde::Serialize;

/// One directive with its arguments and block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyntaxNode {
    pub name: String,
    pub args: Vec<String>,
    /// Where it is written, as `file:line.column-line.column`.
    pub span: String,
    /// The comments on the lines before it and after it on its line.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<String>,
    /// The directives of its block; absent when it ends with `;`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(no_recursion))]
    pub block: Option<Vec<SyntaxNode>>,
}

/// A file's directives as written.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyntaxTree {
    pub file: String,
    pub directives: Vec<SyntaxNode>,
    /// Syntax errors; the tree holds what could be read around them.
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<Object>))]
    pub diagnostics: Vec<Diagnostic>,
}

/// The syntax tree of `file`, or `None` when there is no such file.
pub fn syntax_tree(sources: &Sources, file: &str) -> Option<SyntaxTree> {
    let text = sources.get(file)?;
    let parsed = panel_dsl::parse(file, text);
    let index = LineIndex::new(text);
    let directives = parsed
        .document
        .directives
        .iter()
        .map(|directive| node(directive, file, text, &index))
        .collect();
    Some(SyntaxTree {
        file: file.to_owned(),
        directives,
        diagnostics: parsed.diagnostics,
    })
}

fn node(directive: &Directive, file: &str, text: &str, index: &LineIndex) -> SyntaxNode {
    let comments = directive
        .leading
        .iter()
        .filter_map(|trivia| match trivia {
            Trivia::Comment(comment) => Some(comment.text.trim().to_owned()),
            Trivia::BlankLine => None,
        })
        .chain(
            directive
                .comment
                .iter()
                .map(|comment| comment.text.trim().to_owned()),
        )
        .collect();
    SyntaxNode {
        name: directive.name.value.clone(),
        args: directive.args.iter().map(|arg| arg.value.clone()).collect(),
        span: index.describe(file, text, directive.span),
        comments,
        block: directive.block().map(|block| {
            block
                .directives
                .iter()
                .map(|child| node(child, file, text, index))
                .collect()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directives_keep_their_arguments_blocks_and_comments() {
        let sources = Sources::single(
            "language_version 1;\nhttp {\n    # Shop\n    server shop {\n        proxy app; # main\n    }\n}\n",
        );
        let tree = syntax_tree(&sources, "main.conf").unwrap();
        assert!(tree.diagnostics.is_empty());
        assert_eq!(tree.directives[0].args, ["1"]);
        assert_eq!(tree.directives[0].block, None);
        let server = &tree.directives[1].block.as_ref().unwrap()[0];
        assert_eq!(server.name, "server");
        assert_eq!(server.span, "main.conf:4.5-6.5");
        assert_eq!(server.comments, ["Shop"]);
        let proxy = &server.block.as_ref().unwrap()[0];
        assert_eq!(proxy.args, ["app"]);
        assert_eq!(proxy.comments, ["main"]);
        assert!(syntax_tree(&sources, "other.conf").is_none());
    }
}
