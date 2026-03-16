use tree_sitter::Parser;
use tree_sitter::wasmtime::Engine;
use tree_sitter::WasmStore;

pub fn parse_with_wasm(wasm_bytes: &[u8], source_code: &str) -> Option<tree_sitter::Tree> {
    let engine = Engine::default();
    let mut store = WasmStore::new(&engine).expect("Failed to create WasmStore");
    let language = store.load_language("python", wasm_bytes).expect("Failed to load language");

    let mut parser = Parser::new();
    parser.set_wasm_store(store).expect("Failed to set WasmStore");
    parser.set_language(&language).expect("Failed to set language");

    parser.parse(source_code, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_wasm_parsing_matches_native() {
        let code = "def hello():\n    print(\"Hello world\")\n";
        
        // Native parser
        let mut native_parser = Parser::new();
        native_parser.set_language(&tree_sitter_python::LANGUAGE.into()).expect("Failed to load native python");
        let native_tree = native_parser.parse(code, None).expect("Native parsing failed");
        
        // WASM parser
        let wasm_bytes = fs::read("parsers/tree-sitter-python.wasm").expect("Failed to read wasm file");
        let wasm_tree = parse_with_wasm(&wasm_bytes, code).expect("Wasm parsing failed");
        
        assert_eq!(native_tree.root_node().to_sexp(), wasm_tree.root_node().to_sexp());
    }
}