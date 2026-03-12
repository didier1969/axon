use tree_sitter::{Language, Parser, Query, QueryCursor};

fn main() {
    let language = tree_sitter_python::language();
    let mut parser = Parser::new();
    parser.set_language(language).unwrap();
    println!("Tree-sitter python loaded!");
}
