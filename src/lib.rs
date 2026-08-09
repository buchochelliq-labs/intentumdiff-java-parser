//! Java parser plugin - full-parse mode.
//!
//! Handles `.java` files. Parses source with tree-sitter-java directly.
//! This plugin walks the CST and emits a
//! SemanticNode tree focused on semantically meaningful constructs.

use intentumdiff_plugin_sdk::{
    cst::CstNode,
    hash::structural_hash_with_memo,
    tree::{SemanticNode, SemanticNodeBuilder},
};

wit_bindgen::generate!({
    path: "wit/plugin.wit",
    world: "parser-plugin",
});

use crate::exports::intentdiff::plugin::parser::ExamplePair;
use crate::exports::intentdiff::plugin::parser::Guest;
use crate::exports::intentdiff::plugin::parser::LanguageInfoRecord;
use crate::exports::intentdiff::plugin::parser::ParserMode;

const PLUGIN_METADATA: &str = include_str!("../plugin_metadata.info");

fn language_info_for(ids: Vec<String>) -> Vec<LanguageInfoRecord> {
    let metadata = intentumdiff_plugin_sdk::metadata::parse_plugin_metadata(PLUGIN_METADATA);
    ids.into_iter()
        .map(|language_id| {
            let info = metadata.language_or_default(&language_id);
            LanguageInfoRecord {
                language_id: info.language_id,
                language_name: info.language_name,
                language_short_name: info.language_short_name,
                monaco_language: info.monaco_language,
                default_filename: info.default_filename,
                language_file_extensions: info.language_file_extensions,
                author: metadata.author().to_string(),
                plugin_version: metadata.plugin_version().to_string(),
                last_updated: metadata.last_updated().to_string(),
            }
        })
        .collect()
}
struct JavaParser;

const TRIVIA: &[&str] = &["comment", "line_comment", "block_comment", "whitespace"];

const SEMANTIC_TYPES: &[&str] = &[
    // Root
    "program",
    // Declarations
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "annotation_type_declaration",
    "record_declaration",
    // Members
    "method_declaration",
    "constructor_declaration",
    "field_declaration",
    "constant_declaration",
    "enum_constant",
    "annotation_type_element_declaration",
    // Imports / package
    "import_declaration",
    "package_declaration",
    // Statements
    "local_variable_declaration",
    "expression_statement",
    "return_statement",
    "throw_statement",
    "if_statement",
    "for_statement",
    "enhanced_for_statement",
    "while_statement",
    "do_statement",
    "try_statement",
    "catch_clause",
    "finally_clause",
    "switch_statement",
    "switch_block_statement_group",
    "synchronized_statement",
    "assert_statement",
    "break_statement",
    "continue_statement",
    "labeled_statement",
    // Expressions
    "assignment_expression",
    "method_invocation",
    "object_creation_expression",
    "lambda_expression",
    "method_reference",
    // Types
    "type_identifier",
    "identifier",
    "string_literal",
    "integer_literal",
    "boolean_type",
    // Literals (issue #72): tree-sitter-java's literal kinds are the grammar-specific
    // names below, NOT "integer_literal" — without them `return 99;` prunes the value
    // node and the #70 facts pass can't derive return_kind.
    "decimal_integer_literal",
    "hex_integer_literal",
    "octal_integer_literal",
    "binary_integer_literal",
    "decimal_floating_point_literal",
    "hex_floating_point_literal",
    "character_literal",
    "null_literal",
    "true",
    "false",
    // Modifiers / annotations
    "annotation",
    "modifiers",
];

fn is_semantic(node_type: &str) -> bool {
    SEMANTIC_TYPES.contains(&node_type)
}

fn label_for(node: &CstNode) -> String {
    if node.is_leaf() {
        return node.text_or_empty().to_string();
    }
    // Literal containers label with their captured source text (SDK-shared, issue #47).
    if let Some(label) = intentumdiff_plugin_sdk::ts_convert::literal_label(node) {
        return label;
    }
    match node.node_type.as_str() {
        // Named declarations: first identifier or type_identifier is the name
        "class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "annotation_type_declaration"
        | "record_declaration" => {
            for child in &node.children {
                if child.node_type == "identifier" || child.node_type == "type_identifier" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        // Methods and constructors
        "method_declaration" | "constructor_declaration" => {
            for child in &node.children {
                if child.node_type == "identifier" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        // Imports: use the qualified name
        "import_declaration" | "package_declaration" => {
            for child in &node.children {
                if child.node_type == "scoped_identifier" || child.node_type == "identifier" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        // Annotations
        "annotation" => {
            for child in &node.children {
                if child.node_type == "identifier" || child.node_type == "type_identifier" {
                    return child.text_or_empty().to_string();
                }
            }
        }
        _ => {}
    }
    for child in &node.children {
        if child.node_type == "identifier" {
            return child.text_or_empty().to_string();
        }
    }
    node.node_type.clone()
}

fn is_class_like(node_type: &str) -> bool {
    matches!(
        node_type,
        "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "annotation_type_declaration"
            | "record_declaration"
    )
}

fn is_method_like(node_type: &str) -> bool {
    matches!(node_type, "method_declaration" | "constructor_declaration")
}

fn convert(
    node: &CstNode,
    id_prefix: &str,
    parent_class: Option<&str>,
    memo: &mut std::collections::HashMap<usize, String>,
) -> Option<SemanticNode> {
    convert_semantic_classed(
        node,
        id_prefix,
        parent_class,
        memo,
        &|_| false,
        &is_semantic,
        &is_class_like,
        &is_method_like,
        &label_for,
    )
}



use intentumdiff_plugin_sdk::ts_convert::{convert_semantic_classed, node_to_cst};

fn parse_source(source: &str) -> Result<CstNode, String> {
    let mut parser = tree_sitter::Parser::new();
    let lang = tree_sitter_java::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|_| "Failed to load Java grammar".to_string())?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "Parse failed".to_string())?;
    Ok(node_to_cst(tree.root_node(), source.as_bytes()))
}

fn process_impl(source: &str) -> String {
    let root: CstNode = match parse_source(source) {
        Ok(n) => n,
        Err(e) => return format!(r#"{{"error":"{}"}}"#, e),
    };
    let mut memo: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let sem = match convert(&root, "0", None, &mut memo) {
        Some(n) => n,
        None => return r#"{"error":"Empty semantic tree"}"#.to_string(),
    };
    match serde_json::to_string(&sem) {
        Ok(s) => s,
        Err(e) => format!(r#"{{"error":"Serialisation error: {}"}}"#, e),
    }
}

impl Guest for JavaParser {
    fn get_parser_mode() -> ParserMode {
        ParserMode::FullParse
    }
    fn grammar_id() -> String {
        "java".to_string()
    }
    fn detect_language(filename: String, _content: String) -> String {
        if filename.ends_with(".java") {
            "java".to_string()
        } else {
            String::new()
        }
    }
    fn preprocess_source(source: String) -> String {
        source
    }
    fn example(_language: String) -> ExamplePair {
        ExamplePair {
            old: "public class Calculator {\n    public int add(int a, int b) {\n        return a + b;\n    }\n\n    public int multiply(int a, int b) {\n        return a * b;\n    }\n}\n".to_string(),
            new: "public class Calculator {\n    public int add(int first, int second) {\n        return first + second;\n    }\n\n    public int multiply(int first, int second) {\n        return first * second;\n    }\n\n    public double divide(int dividend, int divisor) {\n        if (divisor == 0) throw new ArithmeticException(\"Division by zero\");\n        return (double) dividend / divisor;\n    }\n}\n".to_string(),
        }
    }
    fn process(input: String, _language: String, _filename: String) -> String {
        process_impl(&input)
    }
    fn trivia_node_types() -> Vec<String> {
        TRIVIA.iter().map(|s| s.to_string()).collect()
    }
    fn language_ids() -> Vec<String> {
        vec!["java".to_string()]
    }
    fn language_info() -> Vec<LanguageInfoRecord> {
        language_info_for(Self::language_ids())
    }
    fn priority() -> i32 {
        0
    }
}

export!(JavaParser);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::intentdiff::plugin::parser::Guest;
    use intentumdiff_plugin_sdk::testing as t;

    #[test]
    fn grammar_id_nonempty() {
        assert!(!JavaParser::grammar_id().is_empty());
    }

    #[test]
    fn language_ids_contain_grammar_id() {
        let gid = JavaParser::grammar_id();
        let ids = JavaParser::language_ids();
        assert!(
            ids.contains(&gid),
            "language_ids {:?} must contain {:?}",
            ids,
            gid
        );
    }

    #[test]
    fn detect_language_known_ext() {
        let r = JavaParser::detect_language("test.java".to_string(), "".to_string());
        assert_eq!(r.as_str(), "java");
    }

    #[test]
    fn detect_language_unknown_ext() {
        let r =
            JavaParser::detect_language("test.xyz_notareal_ext_9z8y".to_string(), "".to_string());
        assert_eq!(r.as_str(), "");
    }

    #[test]
    fn process_impl_empty_returns_valid_json() {
        let out = process_impl("");
        t::assert_valid_json(&out, "process(empty)");
    }

    #[test]
    fn process_impl_whitespace_returns_valid_json() {
        let out = process_impl("   \n  ");
        t::assert_valid_json(&out, "process(whitespace)");
    }
}
