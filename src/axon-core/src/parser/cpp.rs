use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct CppParser {
    wasm_bytes: &'static [u8],
}

impl Default for CppParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CppParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-cpp.wasm"),
        }
    }

    fn walk<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        current_ns: &str,
        is_template: bool,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "preproc_include" => Self::extract_include(child, source_bytes, result),
                "namespace_definition" => {
                    Self::extract_namespace(child, source_bytes, result, current_ns)
                }
                "template_declaration" => {
                    Self::extract_template(child, source_bytes, result, current_ns)
                }
                "alias_declaration" | "type_definition" => {
                    Self::extract_type_alias(child, source_bytes, result, current_ns)
                }
                "function_definition" => {
                    Self::extract_function(child, source_bytes, result, current_ns, is_template)
                }
                "class_specifier" | "struct_specifier" | "enum_specifier" => {
                    Self::extract_class(child, source_bytes, result, current_ns, is_template)
                }
                "call_expression" => Self::extract_call(child, source_bytes, result, ""),
                _ => {
                    Self::extract_module_or_macro_definition(
                        child,
                        source_bytes,
                        result,
                        current_ns,
                    );
                    Self::walk(child, source_bytes, result, current_ns, is_template);
                }
            }
        }
    }

    fn extract_module_or_macro_definition<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        current_ns: &str,
    ) {
        let text = node.utf8_text(source_bytes).unwrap_or("").trim();
        if text.starts_with("export module ")
            || (text.starts_with("module ") && !text.starts_with("module;"))
        {
            let prefix = if text.starts_with("export module ") {
                "export module "
            } else {
                "module "
            };
            let is_export = prefix.starts_with("export");
            if let Some(rest) = text.strip_prefix(prefix) {
                let mod_name = rest.split(';').next().unwrap_or("").trim().to_string();
                if !mod_name.is_empty()
                    && !result
                        .symbols
                        .iter()
                        .any(|s| s.name == mod_name && s.kind == "cpp_module")
                {
                    let mut properties = HashMap::new();
                    if is_export {
                        properties.insert("exported".to_string(), "true".to_string());
                    }
                    result.symbols.push(Symbol {
                        name: mod_name,
                        kind: "cpp_module".to_string(),
                        start_line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        docstring: None,
                        is_entry_point: false,
                        is_public: is_export,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties,
                        embedding: None,
                    });
                }
            }
        } else if text.starts_with("import ") || text.starts_with("export import ") {
            let prefix = if text.starts_with("export import ") {
                "export import "
            } else {
                "import "
            };
            if let Some(rest) = text.strip_prefix(prefix) {
                let imported = rest.split(';').next().unwrap_or("").trim().to_string();
                if !imported.is_empty()
                    && !result
                        .relations
                        .iter()
                        .any(|r| r.to == imported && r.rel_type == "imports")
                {
                    result.relations.push(Relation {
                        from: current_ns.to_string(),
                        to: imported,
                        rel_type: "imports".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        } else if text.starts_with("DUCKDB_EXTENSION_ENTRYPOINT") {
            if let Some(open) = text.find('(') {
                let after_open = &text[open + 1..];
                if let Some(comma_or_close) = after_open.find(|c| c == ',' || c == ')') {
                    let ext_name = after_open[..comma_or_close].trim().to_string();
                    if !ext_name.is_empty() && !result.symbols.iter().any(|s| s.name == ext_name) {
                        let mut properties = HashMap::new();
                        properties.insert("duckdb_extension".to_string(), "true".to_string());
                        result.symbols.push(Symbol {
                            name: ext_name.clone(),
                            kind: "duckdb_extension".to_string(),
                            start_line: node.start_position().row + 1,
                            end_line: node.end_position().row + 1,
                            docstring: None,
                            is_entry_point: true,
                            is_public: true,
                            tested: false,
                            is_nif: true,
                            is_unsafe: true,
                            properties,
                            embedding: None,
                        });
                        Self::walk_for_calls(node, source_bytes, result, &ext_name);
                    }
                }
            }
        }
    }

    fn extract_include<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        if let Some(path_node) = Self::find_child_by_type(node, "system_lib_string")
            .or_else(|| Self::find_child_by_type(node, "string_literal"))
        {
            let path = path_node
                .utf8_text(source_bytes)
                .unwrap_or("")
                .trim()
                .to_string();
            if !path.is_empty() {
                result.relations.push(Relation {
                    from: "".to_string(),
                    to: path,
                    rel_type: "includes".to_string(),
                    properties: HashMap::new(),
                });
            }
        }
    }

    fn extract_namespace<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        current_ns: &str,
    ) {
        let mut ns_name = String::new();
        if let Some(name_node) = Self::find_child_by_type(node, "namespace_identifier")
            .or_else(|| Self::find_child_by_type(node, "identifier"))
            .or_else(|| Self::find_child_by_type(node, "nested_namespace_specifier"))
        {
            ns_name = name_node.utf8_text(source_bytes).unwrap_or("").to_string();
        }

        let full_ns = if current_ns.is_empty() {
            ns_name.clone()
        } else if !ns_name.is_empty() {
            format!("{}::{}", current_ns, ns_name)
        } else {
            current_ns.to_string()
        };

        if !ns_name.is_empty() {
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;
            result.symbols.push(Symbol {
                name: full_ns.clone(),
                kind: "namespace".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        if let Some(body) = Self::find_child_by_type(node, "declaration_list") {
            Self::walk(body, source_bytes, result, &full_ns, false);
        }
    }

    fn extract_template<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        current_ns: &str,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "template_parameter_list" {
                continue;
            }
            match child.kind() {
                "function_definition" => {
                    Self::extract_function(child, source_bytes, result, current_ns, true)
                }
                "class_specifier" | "struct_specifier" => {
                    Self::extract_class(child, source_bytes, result, current_ns, true)
                }
                "alias_declaration" | "type_definition" => {
                    Self::extract_type_alias(child, source_bytes, result, current_ns)
                }
                _ => Self::walk(child, source_bytes, result, current_ns, true),
            }
        }
    }

    fn extract_type_alias<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        current_ns: &str,
    ) {
        if let Some(name_node) = Self::find_child_by_type(node, "type_identifier") {
            let name = name_node.utf8_text(source_bytes).unwrap_or("").to_string();
            if !name.is_empty() {
                let start_line = node.start_position().row + 1;
                let end_line = node.end_position().row + 1;
                let mut properties = HashMap::new();
                if !current_ns.is_empty() {
                    properties.insert("namespace".to_string(), current_ns.to_string());
                }
                result.symbols.push(Symbol {
                    name,
                    kind: "type_alias".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties,
                    embedding: None,
                });
            }
        }
    }

    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity, base 1 +
    /// one per decision point. C++ lambdas are not extracted as a separate
    /// Symbol in this parser, so their branches fold into the enclosing
    /// function — no nested-exclusion guard needed.
    const BRANCHING_KINDS: &[&str] = &[
        "if_statement",
        "for_statement",
        "for_range_loop",
        "while_statement",
        "do_statement",
        "case_statement",
        "catch_clause",
        "conditional_expression",
    ];

    fn count_branches(node: Node) -> i32 {
        let mut count = 0i32;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Self::BRANCHING_KINDS.contains(&child.kind()) {
                count += 1;
            }
            count += Self::count_branches(child);
        }
        count
    }

    fn extract_function<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        current_ns: &str,
        is_template: bool,
    ) {
        let mut name = String::new();
        if let Some(decl) = Self::find_child_by_type(node, "function_declarator") {
            if let Some(id) = Self::find_child_by_type(decl, "identifier")
                .or_else(|| Self::find_child_by_type(decl, "field_identifier"))
            {
                name = id.utf8_text(source_bytes).unwrap_or("").to_string();
            }
        }

        let node_content = node.utf8_text(source_bytes).unwrap_or("");
        if node_content.contains("DUCKDB_EXTENSION_ENTRYPOINT") {
            if let Some(start) = node_content.find("DUCKDB_EXTENSION_ENTRYPOINT") {
                if let Some(open) = node_content[start..].find('(') {
                    let after_open = &node_content[start + open + 1..];
                    if let Some(comma_or_close) = after_open.find(|c| c == ',' || c == ')') {
                        let ext = after_open[..comma_or_close].trim();
                        if !ext.is_empty() {
                            name = ext.to_string();
                        }
                    }
                }
            }
        } else if name.is_empty()
            && (node_content.contains("__global__")
                || node_content.contains("__device__")
                || node_content.contains("__host__"))
        {
            if let Some(open_paren) = node_content.find('(') {
                let prefix = node_content[..open_paren].trim();
                if let Some(last_word) = prefix.split_whitespace().last() {
                    let cleaned = last_word.trim_start_matches('*');
                    if !cleaned.is_empty()
                        && cleaned.chars().all(|c| c.is_alphanumeric() || c == '_')
                    {
                        name = cleaned.to_string();
                    }
                }
            }
        }

        if !name.is_empty() {
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            let mut is_nif = false;
            if node_content.contains("JNIEXPORT")
                || node_content.contains("JNICALL")
                || node_content.contains("__declspec(dllexport)")
                || node_content.contains("extern \"C\"")
                || node_content.contains("PHP_FUNCTION")
                || node_content.contains("PHP_METHOD")
                || node_content.contains("rb_define_method")
                || node_content.contains("Init_")
                || node_content.contains("PyMODINIT_FUNC")
                || node_content.contains("ERL_NIF_INIT")
            {
                is_nif = true;
            }

            let mut properties = HashMap::new();
            if !current_ns.is_empty() {
                properties.insert("namespace".to_string(), current_ns.to_string());
            }
            if is_template {
                properties.insert("is_template".to_string(), "true".to_string());
            }

            let mut kind = "function".to_string();
            let mut is_entry_point = is_nif;

            if node_content.contains("__global__") {
                kind = "kernel".to_string();
                is_entry_point = true;
                properties.insert("cuda_execution_space".to_string(), "global".to_string());
                properties.insert("gpu_kernel".to_string(), "true".to_string());
            } else if node_content.contains("__device__") {
                properties.insert("cuda_execution_space".to_string(), "device".to_string());
                properties.insert("gpu_kernel".to_string(), "true".to_string());
            } else if node_content.contains("__host__") {
                properties.insert("cuda_execution_space".to_string(), "host".to_string());
            }

            if node_content.contains("co_await")
                || node_content.contains("co_yield")
                || node_content.contains("co_return")
            {
                properties.insert("is_coroutine".to_string(), "true".to_string());
                properties.insert("coroutine".to_string(), "true".to_string());
            }

            if node_content.contains("DUCKDB_EXTENSION_ENTRYPOINT")
                || (name.ends_with("_init") && node_content.contains("duckdb"))
            {
                properties.insert("duckdb_extension".to_string(), "true".to_string());
                is_entry_point = true;
                is_nif = true;
            }

            if node_content.contains("parallel_for")
                || node_content.contains("sycl::")
                || node_content.contains("sycl::queue")
            {
                properties.insert("gpu_offload".to_string(), "sycl".to_string());
            }

            if let Some(body) = Self::find_child_by_type(node, "compound_statement") {
                // REQ-AXO-91506 — propagate caller name into call extraction.
                Self::walk_for_calls(body, source_bytes, result, &name);
                let complexity = 1 + Self::count_branches(body);
                properties.insert("cyclomatic_complexity".to_string(), complexity.to_string());
            }

            result.symbols.push(Symbol {
                name,
                kind,
                start_line,
                end_line,
                docstring: None,
                is_entry_point,
                is_public: true,
                tested: false,
                is_nif,
                is_unsafe: true,
                properties,
                embedding: None,
            });
        }
    }

    fn extract_class<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        current_ns: &str,
        is_template: bool,
    ) {
        if let Some(name_node) = Self::find_child_by_type(node, "type_identifier") {
            let name = name_node.utf8_text(source_bytes).unwrap_or("").to_string();
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            let mut properties = HashMap::new();
            if !current_ns.is_empty() {
                properties.insert("namespace".to_string(), current_ns.to_string());
            }
            if is_template {
                properties.insert("is_template".to_string(), "true".to_string());
            }

            result.symbols.push(Symbol {
                name: name.clone(),
                kind: "class".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties,
                embedding: None,
            });

            if let Some(body) = Self::find_child_by_type(node, "field_declaration_list") {
                Self::walk(body, source_bytes, result, current_ns, false);
            }
        }
    }

    fn extract_call<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        caller: &str,
    ) {
        if let Some(func_node) = node.named_child(0) {
            let call_name = func_node.utf8_text(source_bytes).unwrap_or("").to_string();
            if !call_name.is_empty() {
                let mut rel_type = "calls".to_string();
                if call_name.contains("parallel_for") || call_name.contains("single_task") {
                    rel_type = "dispatches_kernel".to_string();
                } else if call_name.contains("CreateScalarFunction")
                    || call_name.contains("CreateTableFunction")
                    || call_name.contains("CreateVectorizedFunction")
                    || call_name.contains("CreateAggregateFunction")
                {
                    rel_type = "registers_duckdb_udf".to_string();
                }

                result.relations.push(Relation {
                    from: caller.to_string(),
                    to: call_name,
                    rel_type,
                    properties: HashMap::new(),
                });
            }
        }
        Self::walk_for_calls(node, source_bytes, result, caller);
    }

    fn walk_for_calls<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        caller: &str,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "call_expression" {
                Self::extract_call(child, source_bytes, result, caller);
            } else {
                Self::walk_for_calls(child, source_bytes, result, caller);
            }
        }
    }

    fn find_child_by_type<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == kind {
                return Some(child);
            }
        }
        None
    }
}

impl Parser for CppParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        if let Some(tree) = parse_with_wasm_safe("cpp", self.wasm_bytes, content) {
            Self::walk(tree.root_node(), content.as_bytes(), &mut result, "", false);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    //! REQ-AXO-902185 (god-objects) — cyclomatic complexity regression tests.
    use super::*;

    fn parser() -> CppParser {
        CppParser::new()
    }

    #[test]
    fn simple_function_has_complexity_one() {
        let result = parser().parse("int f() { int x = 1; return x; }");
        if result.symbols.is_empty() {
            eprintln!("cpp wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        assert_eq!(
            f.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn branching_function_counts_each_decision_point() {
        let result = parser().parse(
            "int f(int x) { \
                if (x > 0) { return 1; } \
                for (int i : {1,2,3}) {} \
                try { } catch (int e) { } \
                return x > 0 ? 1 : 0; \
            }",
        );
        if result.symbols.is_empty() {
            eprintln!("cpp wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        // base 1 + if + range-for + catch + ternary = 5
        assert_eq!(
            f.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("5")
        );
    }

    #[test]
    fn cpp_parses_includes_namespaces_aliases_and_templates() {
        let code = r#"
        #include <vector>
        #include "engine/core.hpp"

        namespace engine {
            using ByteVector = std::vector<uint8_t>;
            typedef unsigned long ulong;

            template <typename T>
            class Buffer {
            public:
                void reset() {}
            };

            void run() {
                Buffer<int> b;
                b.reset();
            }
        }
        "#;
        let result = parser().parse(code);
        if result.symbols.is_empty() && result.relations.is_empty() {
            eprintln!("cpp wasm grammar unavailable, skipping");
            return;
        }

        // Check includes
        assert!(result
            .relations
            .iter()
            .any(|r| r.to == "<vector>" && r.rel_type == "includes"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.to == "\"engine/core.hpp\"" && r.rel_type == "includes"));

        // Check namespace
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "engine" && s.kind == "namespace"));

        // Check type alias
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "ByteVector" && s.kind == "type_alias"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "ulong" && s.kind == "type_alias"));

        // Check template class
        let buffer = result
            .symbols
            .iter()
            .find(|s| s.name == "Buffer" && s.kind == "class");
        assert!(buffer.is_some());
        assert_eq!(
            buffer
                .unwrap()
                .properties
                .get("is_template")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(
            buffer
                .unwrap()
                .properties
                .get("namespace")
                .map(String::as_str),
            Some("engine")
        );

        // Check enclosed function
        let run = result
            .symbols
            .iter()
            .find(|s| s.name == "run" && s.kind == "function");
        assert!(run.is_some());
        assert_eq!(
            run.unwrap().properties.get("namespace").map(String::as_str),
            Some("engine")
        );
    }

    #[test]
    fn test_cuda_kernels_and_execution_spaces() {
        let cuda_code = r#"
        __global__ void matmul_kernel(float* a, float* b, float* c, int n) {
            int idx = blockIdx.x * blockDim.x + threadIdx.x;
            if (idx < n) {
                c[idx] = a[idx] * b[idx];
            }
        }

        __device__ float helper_device_func(float x) {
            return x * 2.0f;
        }

        __host__ void launch(float* a, float* b, float* c, int n) {
            matmul_kernel<<<128, 256>>>(a, b, c, n);
        }
        "#;
        let result = parser().parse(cuda_code);
        let kernel = result
            .symbols
            .iter()
            .find(|s| s.name == "matmul_kernel")
            .expect("matmul_kernel symbol");
        assert_eq!(kernel.kind, "kernel");
        assert!(kernel.is_entry_point);
        assert_eq!(
            kernel
                .properties
                .get("cuda_execution_space")
                .map(String::as_str),
            Some("global")
        );
        assert_eq!(
            kernel.properties.get("gpu_kernel").map(String::as_str),
            Some("true")
        );

        let dev_fn = result
            .symbols
            .iter()
            .find(|s| s.name == "helper_device_func")
            .expect("helper_device_func symbol");
        assert_eq!(
            dev_fn
                .properties
                .get("cuda_execution_space")
                .map(String::as_str),
            Some("device")
        );
        assert_eq!(
            dev_fn.properties.get("gpu_kernel").map(String::as_str),
            Some("true")
        );

        let host_fn = result
            .symbols
            .iter()
            .find(|s| s.name == "launch")
            .expect("launch symbol");
        assert_eq!(
            host_fn
                .properties
                .get("cuda_execution_space")
                .map(String::as_str),
            Some("host")
        );
    }

    #[test]
    fn test_sycl_parallel_for_dispatch() {
        let sycl_code = r#"
        void run_sycl(sycl::queue& q, float* data, int n) {
            q.parallel_for(sycl::range<1>(n), [=](sycl::id<1> idx) {
                data[idx] = data[idx] + 1.0f;
            });
        }
        "#;
        let result = parser().parse(sycl_code);
        let fn_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "run_sycl")
            .expect("run_sycl symbol");
        assert_eq!(
            fn_sym.properties.get("gpu_offload").map(String::as_str),
            Some("sycl")
        );
        assert!(result
            .relations
            .iter()
            .any(|r| r.rel_type == "dispatches_kernel" && r.to.contains("parallel_for")));
    }

    #[test]
    fn test_cpp20_modules_and_imports() {
        let module_code = r#"
        export module math.tensor;

        import math.vector;
        import std.core;

        export int compute_norm() {
            return 42;
        }
        "#;
        let result = parser().parse(module_code);
        let mod_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "math.tensor")
            .expect("module symbol");
        assert_eq!(mod_sym.kind, "cpp_module");
        assert!(mod_sym.is_public);

        assert!(result
            .relations
            .iter()
            .any(|r| r.rel_type == "imports" && r.to == "math.vector"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.rel_type == "imports" && r.to == "std.core"));
    }

    #[test]
    fn test_cpp20_coroutines() {
        let coro_code = r#"
        generator<int> generate_sequence(int count) {
            for (int i = 0; i < count; ++i) {
                co_yield i;
            }
            co_return;
        }

        task<void> async_task() {
            co_await fetch_data();
        }
        "#;
        let result = parser().parse(coro_code);
        let gen_fn = result
            .symbols
            .iter()
            .find(|s| s.name == "generate_sequence")
            .expect("generate_sequence symbol");
        assert_eq!(
            gen_fn.properties.get("is_coroutine").map(String::as_str),
            Some("true")
        );

        let task_fn = result
            .symbols
            .iter()
            .find(|s| s.name == "async_task")
            .expect("async_task symbol");
        assert_eq!(
            task_fn.properties.get("is_coroutine").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn test_duckdb_extension_entrypoint_and_udf() {
        let duckdb_code = r#"
        DUCKDB_EXTENSION_ENTRYPOINT(custom_ext, db) {
            con.CreateScalarFunction("custom_scalar", &ScalarFunction);
            con.CreateTableFunction("custom_table", &TableFunction);
        }
        "#;
        let result = parser().parse(duckdb_code);
        let ext_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "custom_ext" || s.name.contains("custom_ext"))
            .expect("duckdb extension symbol");
        assert!(ext_sym.is_entry_point);
        assert_eq!(
            ext_sym
                .properties
                .get("duckdb_extension")
                .map(String::as_str),
            Some("true")
        );
        assert!(result
            .relations
            .iter()
            .any(|r| r.rel_type == "registers_duckdb_udf"));
    }
}
