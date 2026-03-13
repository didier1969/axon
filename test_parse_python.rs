use tree_sitter::{Parser, Language};

fn main() {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let code = r#"
        #[rustler::nif]
        pub fn compute_hash(data: String) -> String {
            "hash".to_string()
        }
    "#;
    let tree = parser.parse(code, None).unwrap();
    println!("{}", tree.root_node().to_sexp());
}