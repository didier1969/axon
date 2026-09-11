use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use tree_sitter::Node;

static SQLA_FK_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"ForeignKey\(\s*["']([^"']+)["']\s*\)"#).unwrap());
static SQLA_REL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"relationship\(\s*(?:argument=\s*)?["']?([A-Za-z0-9_]+)["']?"#).unwrap()
});

pub struct PythonParser {
    wasm_bytes: &'static [u8],
}

impl PythonParser {
    fn block_split_lines<'a>(&self, block: Node<'a>) -> Vec<usize> {
        let mut cursor = block.walk();
        block
            .named_children(&mut cursor)
            .map(|child| child.start_position().row + 1)
            .collect()
    }

    // REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity, base 1 +
    // one per decision point. Nested `function_definition`/`lambda` skipped:
    // they get their own count when `walk` visits them separately, so a
    // nested closure's branches must never inflate the enclosing function's.
    fn count_branches(&self, node: Node) -> i32 {
        let mut count = 0i32;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if matches!(child.kind(), "function_definition" | "lambda") {
                continue;
            }
            if matches!(
                child.kind(),
                "if_statement"
                    | "elif_clause"
                    | "for_statement"
                    | "while_statement"
                    | "except_clause"
                    | "case_clause"
                    | "conditional_expression"
            ) {
                count += 1;
            }
            count += self.count_branches(child);
        }
        count
    }

    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-python.wasm"),
        }
    }

    fn walk<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, scope: &str) {
        let kind = node.kind();

        match kind {
            "class_definition" => self.extract_class(node, source, result, scope),
            "function_definition" => self.extract_function(node, source, result, scope),
            "call" => self.extract_call(node, source, result, scope),
            "import_statement" | "import_from_statement" => {
                self.extract_import(node, source, result)
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.walk(child, source, result, scope);
                }
            }
        }
    }

    #[allow(clippy::manual_find)]
    fn find_child_by_type<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == kind {
                return Some(child);
            }
        }
        None
    }

    fn find_last_child_by_type<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        let mut last = None;
        for child in node.children(&mut cursor) {
            if child.kind() == kind {
                last = Some(child);
            }
        }
        last
    }

    fn extract_class<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        _scope: &str,
    ) {
        let name_node = self.find_child_by_type(node, "identifier");
        let name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        let is_test =
            name.starts_with("Test") || name.ends_with("Test") || name.ends_with("TestCase");

        result.symbols.push(Symbol {
            name: name.clone(),
            kind: "class".to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: false,
            is_public: !name.starts_with("_"),
            tested: is_test,
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        });

        // Parse base classes (extends)
        if let Some(args) = self.find_child_by_type(node, "argument_list") {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                if child.kind() == "identifier" || child.kind() == "attribute" {
                    let base_name = child.utf8_text(source).unwrap_or("").to_string();
                    result.relations.push(Relation {
                        from: name.clone(),
                        to: base_name,
                        rel_type: "extends".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        if let Some(body) = self.find_child_by_type(node, "block") {
            self.extract_sqlalchemy_attributes(body, source, result, &name);
            self.walk(body, source, result, &name);
        }
    }

    fn extract_sqlalchemy_attributes<'a>(
        &self,
        body: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        class_name: &str,
    ) {
        let mut cursor = body.walk();
        for stmt in body.children(&mut cursor) {
            let assign_node = if stmt.kind() == "assignment" {
                Some(stmt)
            } else if stmt.kind() == "expression_statement" {
                self.find_child_by_type(stmt, "assignment")
            } else {
                None
            };

            let Some(assign) = assign_node else {
                continue;
            };

            let left = assign.child_by_field_name("left");
            let right = assign.child_by_field_name("right");
            let (Some(left_node), Some(right_node)) = (left, right) else {
                continue;
            };

            let left_text = left_node.utf8_text(source).unwrap_or("").trim();
            let var_name = left_text
                .split(':')
                .next()
                .unwrap_or(left_text)
                .trim()
                .to_string();
            let right_text = right_node.utf8_text(source).unwrap_or("").trim();

            if var_name == "__tablename__" {
                let table_name = right_text.trim_matches('"').trim_matches('\'').to_string();
                if let Some(cls_sym) = result
                    .symbols
                    .iter_mut()
                    .find(|s| s.name == class_name && s.kind == "class")
                {
                    cls_sym
                        .properties
                        .insert("table".to_string(), table_name.clone());
                }
                let mut p = HashMap::new();
                p.insert("orm".to_string(), "sqlalchemy".to_string());
                p.insert("table".to_string(), table_name.clone());
                result.relations.push(Relation {
                    from: class_name.to_string(),
                    to: table_name,
                    rel_type: "references".to_string(),
                    properties: p,
                });
                continue;
            }

            let is_col = right_text.contains("Column(") || right_text.contains("mapped_column(");
            let is_rel = right_text.contains("relationship(");

            if is_col || is_rel {
                let mut field_props = HashMap::new();
                if is_col {
                    field_props.insert("column".to_string(), "true".to_string());
                }
                if is_rel {
                    field_props.insert("relation".to_string(), "relationship".to_string());
                }

                let full_field_name = format!("{}.{}", class_name, var_name);
                result.symbols.push(Symbol {
                    name: full_field_name.clone(),
                    kind: "field".to_string(),
                    start_line: stmt.start_position().row + 1,
                    end_line: stmt.end_position().row + 1,
                    docstring: None,
                    is_entry_point: false,
                    is_public: !var_name.starts_with('_'),
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: field_props,
                    embedding: None,
                });

                result.relations.push(Relation {
                    from: class_name.to_string(),
                    to: full_field_name,
                    rel_type: "contains".to_string(),
                    properties: HashMap::new(),
                });
            }

            if let Some(fk_cap) = SQLA_FK_RE.captures(right_text) {
                if let Some(fk_target) = fk_cap.get(1) {
                    let fk_str = fk_target.as_str();
                    let target_table = fk_str.split('.').next().unwrap_or(fk_str).to_string();

                    let mut props = HashMap::new();
                    props.insert("orm".to_string(), "sqlalchemy".to_string());
                    props.insert("foreign_key".to_string(), fk_str.to_string());
                    props.insert("field".to_string(), var_name.clone());

                    result.relations.push(Relation {
                        from: class_name.to_string(),
                        to: target_table,
                        rel_type: "references".to_string(),
                        properties: props,
                    });
                }
            }

            if is_rel {
                if let Some(rel_cap) = SQLA_REL_RE.captures(right_text) {
                    if let Some(target_cap) = rel_cap.get(1) {
                        let target_cls = target_cap.as_str().to_string();
                        let mut props = HashMap::new();
                        props.insert("orm".to_string(), "sqlalchemy".to_string());
                        props.insert("relation".to_string(), "relationship".to_string());
                        props.insert("field".to_string(), var_name);

                        result.relations.push(Relation {
                            from: class_name.to_string(),
                            to: target_cls,
                            rel_type: "references".to_string(),
                            properties: props,
                        });
                    }
                }
            }
        }
    }

    fn is_ioc_decorator(dec_text: &str) -> bool {
        let t = dec_text.trim().trim_start_matches('@').trim();
        t.starts_with("api.depends")
            || t.starts_with("api.constrains")
            || t.starts_with("api.onchange")
            || t.starts_with("api.model_create_multi")
            || t.starts_with("api.ondelete")
            || t.starts_with("api.autovacuum")
            || t.starts_with("api.model")
            || t.starts_with("api.returns")
            || t.starts_with("task")
            || t.starts_with("shared_task")
            || t.contains(".task")
            || t.starts_with("receiver")
            || t.contains(".receiver")
            || t.contains(".route")
            || t.contains(".get(")
            || t.contains(".post(")
            || t.contains(".put(")
            || t.contains(".delete(")
            || t.starts_with("click.command")
            || t.starts_with("click.group")
    }

    fn extract_function<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        scope: &str,
    ) {
        let name_node = self.find_child_by_type(node, "identifier");
        let func_name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        // If it's in a class, it's a method
        let is_method = !scope.is_empty();

        let mut props = HashMap::new();
        if is_method {
            props.insert("parent_class".to_string(), scope.to_string());
        }

        let full_name = if is_method {
            format!("{}.{}", scope, func_name)
        } else {
            func_name.clone()
        };

        // Determine if it's a test function
        let is_test = func_name.starts_with("test_")
            || func_name.ends_with("_test")
            || (is_method
                && (scope.starts_with("Test")
                    || scope.ends_with("Test")
                    || scope.ends_with("TestCase"))
                && !func_name.starts_with('_'));

        // Find decorators
        // REQ-AXO-901958 — recognise pytest fixtures (`@fixture` / `@pytest.fixture`
        // / `@pytest_asyncio.fixture`). A fixture is invoked by the test framework,
        // never by an explicit call expression, so it has no inbound CALLS edge and
        // was mis-reported as dead code. We fold it into the already-persisted
        // `tested` flag (which dead_code_count / orphan_code_symbols already skip),
        // avoiding a new Symbol column on the COPY-BINARY ingestion path.
        // REQ-AXO-902330 — recognise IoC framework decorators (Odoo @api.*, Celery @task,
        // Django @receiver, Flask/FastAPI @*.route / @*.get). These methods are invoked by
        // the framework runtime and represent canonical entry points.
        let mut is_fixture = false;
        let mut is_ioc_entry = false;
        let mut is_worker = false;
        let mut is_websocket = false;
        if let Some(parent) = node.parent() {
            if parent.kind() == "decorated_definition" {
                let mut cursor = parent.walk();
                for child in parent.children(&mut cursor) {
                    if child.kind() == "decorator" {
                        let dec_text = child.utf8_text(source).unwrap_or("");
                        if dec_text.contains("fixture") {
                            is_fixture = true;
                        }
                        if Self::is_ioc_decorator(dec_text) {
                            is_ioc_entry = true;
                            props.insert("framework_ioc".to_string(), "true".to_string());
                        }
                        if dec_text.contains(".task")
                            || dec_text.starts_with("@task")
                            || dec_text.starts_with("@shared_task")
                        {
                            is_worker = true;
                            if let Some(q_pos) = dec_text.find("queue=") {
                                let rest = &dec_text[q_pos + 6..];
                                let quote = rest.chars().next().unwrap_or('"');
                                if quote == '"' || quote == '\'' {
                                    let inner = &rest[1..];
                                    if let Some(end_q) = inner.find(quote) {
                                        props.insert(
                                            "queue".to_string(),
                                            inner[..end_q].to_string(),
                                        );
                                    }
                                }
                            }
                        }
                        if dec_text.contains(".websocket(") {
                            is_websocket = true;
                            if let Some(p_pos) = dec_text.find(".websocket(") {
                                let rest = &dec_text[p_pos + 11..];
                                let quote = rest.chars().next().unwrap_or('"');
                                if quote == '"' || quote == '\'' {
                                    let inner = &rest[1..];
                                    if let Some(end_q) = inner.find(quote) {
                                        props.insert(
                                            "route".to_string(),
                                            inner[..end_q].to_string(),
                                        );
                                    }
                                }
                            }
                        }
                        if let Some(id) = self.find_child_by_type(child, "identifier") {
                            let dec_name = id.utf8_text(source).unwrap_or("").to_string();
                            props.insert(format!("decorator_{}", dec_name), "true".to_string());
                        }
                    }
                }
            }
        }

        if let Some(body) = self.find_child_by_type(node, "block") {
            props.insert(
                "header_end_line".to_string(),
                body.start_position().row.to_string(),
            );
            props.insert(
                "body_start_line".to_string(),
                body.start_position().row.saturating_add(1).to_string(),
            );
            props.insert(
                "body_end_line".to_string(),
                body.end_position().row.saturating_add(1).to_string(),
            );
            let split_lines = self.block_split_lines(body);
            if split_lines.len() > 1 {
                props.insert(
                    "body_split_lines".to_string(),
                    split_lines
                        .into_iter()
                        .map(|line| line.to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            let complexity = 1 + self.count_branches(body);
            props.insert("cyclomatic_complexity".to_string(), complexity.to_string());
        }

        // --- UNSAFE DETECTION ---
        let mut is_unsafe = false;
        let mut is_nif = false;
        let body_text = node.utf8_text(source).unwrap_or("");
        if body_text.contains("eval(")
            || body_text.contains("exec(")
            || body_text.contains("os.system(")
            || body_text.contains("subprocess.run(")
        {
            is_unsafe = true;
        }
        if body_text.contains("ctypes") || body_text.contains("cffi") {
            is_nif = true;
        }

        result.symbols.push(Symbol {
            name: full_name.clone(),
            kind: if is_worker {
                "worker".to_string()
            } else if is_websocket {
                "websocket_endpoint".to_string()
            } else if is_method {
                "method".to_string()
            } else {
                "function".to_string()
            },
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: func_name == "main"
                || is_nif
                || is_ioc_entry
                || is_worker
                || is_websocket,
            is_public: !func_name.starts_with("_") || func_name == "__init__",
            // REQ-AXO-901958 — fixtures fold into `tested` (framework-invoked, no
            // inbound CALLS edge → would be mis-flagged as dead).
            tested: is_test || is_fixture,
            is_nif,
            is_unsafe,
            properties: props,
            embedding: None,
        });

        // REQ-AXO-902423 — emit Symbol -> Symbol CONTAINS relation (class -> method).
        if is_method {
            result.relations.push(Relation {
                from: scope.to_string(),
                to: full_name.clone(),
                rel_type: "contains".to_string(),
                properties: HashMap::new(),
            });
        }

        // Link test function to original function if applicable
        if is_test {
            let target = func_name.trim_start_matches("test_").to_string();
            result.relations.push(Relation {
                from: full_name.clone(),
                to: target,
                rel_type: "tests".to_string(),
                properties: HashMap::new(),
            });
        }

        if let Some(body) = self.find_child_by_type(node, "block") {
            self.walk(body, source, result, &full_name);
        }
    }

    fn extract_call<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        scope: &str,
    ) {
        if scope.is_empty() {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                self.walk(child, source, result, scope);
            }
            return;
        }

        let func_node = self
            .find_child_by_type(node, "identifier")
            .or_else(|| self.find_child_by_type(node, "attribute"));

        if let Some(n) = func_node {
            if n.kind() == "attribute" {
                let attr_node = n
                    .child_by_field_name("attribute")
                    .or_else(|| self.find_last_child_by_type(n, "identifier"));
                let obj_node = n.child_by_field_name("object").or_else(|| n.child(0));

                let callee_name = attr_node
                    .and_then(|a| a.utf8_text(source).ok())
                    .unwrap_or("")
                    .to_string();
                let receiver = obj_node
                    .and_then(|o| o.utf8_text(source).ok())
                    .unwrap_or("")
                    .to_string();

                if !callee_name.is_empty() {
                    let mut props = HashMap::new();
                    if !receiver.is_empty() {
                        props.insert("receiver".to_string(), receiver.clone());
                    }
                    result.relations.push(Relation {
                        from: scope.to_string(),
                        to: callee_name.clone(),
                        rel_type: "calls".to_string(),
                        properties: props,
                    });

                    if (callee_name == "delay" || callee_name == "apply_async")
                        && !receiver.is_empty()
                    {
                        result.relations.push(Relation {
                            from: scope.to_string(),
                            to: receiver.clone(),
                            rel_type: "dispatches_job".to_string(),
                            properties: HashMap::new(),
                        });
                    }

                    if callee_name == "send_and_wait"
                        || callee_name == "send"
                        || callee_name == "publish"
                    {
                        if let Some(args) = self.find_child_by_type(node, "argument_list") {
                            let mut ac = args.walk();
                            for arg in args.children(&mut ac) {
                                if arg.kind() == "string" {
                                    let topic_name = arg
                                        .utf8_text(source)
                                        .unwrap_or("")
                                        .trim_matches('"')
                                        .trim_matches('\'')
                                        .to_string();
                                    if !topic_name.is_empty() {
                                        let start_line = node.start_position().row + 1;
                                        let end_line = node.end_position().row + 1;
                                        if !result
                                            .symbols
                                            .iter()
                                            .any(|s| s.kind == "topic" && s.name == topic_name)
                                        {
                                            result.symbols.push(Symbol {
                                                name: topic_name.clone(),
                                                kind: "topic".to_string(),
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
                                        result.relations.push(Relation {
                                            from: scope.to_string(),
                                            to: topic_name,
                                            rel_type: "publishes_to".to_string(),
                                            properties: HashMap::new(),
                                        });
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                let call_name = n.utf8_text(source).unwrap_or("").to_string();
                result.relations.push(Relation {
                    from: scope.to_string(),
                    to: call_name,
                    rel_type: "calls".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        if let Some(args) = self.find_child_by_type(node, "argument_list") {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                if child.kind() == "keyword_argument" {
                    self.extract_framework_keyword_arg(child, source, result, scope);
                }
                self.walk(child, source, result, scope);
            }
        }
    }

    fn extract_framework_keyword_arg<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        scope: &str,
    ) {
        let name_node = node.child_by_field_name("name").or_else(|| node.child(0));
        let val_node = node.child_by_field_name("value").or_else(|| {
            let mut cursor = node.walk();
            node.children(&mut cursor).last()
        });
        let (Some(name_n), Some(val_n)) = (name_node, val_node) else {
            return;
        };
        let key = name_n.utf8_text(source).unwrap_or("");
        if !matches!(
            key,
            "compute" | "inverse" | "search" | "default" | "selection"
        ) {
            return;
        }
        let val_text = val_n.utf8_text(source).unwrap_or("").trim();
        let target_name = val_text.trim_matches(|c| c == '\'' || c == '"');
        if target_name.is_empty() || target_name.contains(' ') || target_name.contains('\n') {
            return;
        }
        let from_target = if scope.is_empty() {
            "module".to_string()
        } else {
            scope.to_string()
        };
        let to_target = if !scope.is_empty() && !target_name.contains('.') {
            format!("{}.{}", scope, target_name)
        } else {
            target_name.to_string()
        };
        result.relations.push(Relation {
            from: from_target,
            to: to_target,
            rel_type: "framework_invokes".to_string(),
            properties: HashMap::new(),
        });
    }

    fn extract_import<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        let from_module = node
            .child_by_field_name("module_name")
            .and_then(|m| m.utf8_text(source).ok())
            .map(|s| s.trim().to_string());

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "aliased_import" => {
                    let name = child
                        .child_by_field_name("name")
                        .and_then(|n| n.utf8_text(source).ok())
                        .map(str::trim);
                    let alias = child
                        .child_by_field_name("alias")
                        .and_then(|a| a.utf8_text(source).ok())
                        .map(str::trim);

                    if let (Some(orig), Some(al)) = (name, alias) {
                        let module = from_module.clone().unwrap_or_else(|| orig.to_string());
                        let mut props = HashMap::new();
                        props.insert("module".to_string(), module);
                        props.insert("alias".to_string(), al.to_string());
                        props.insert("original".to_string(), orig.to_string());

                        result.relations.push(Relation {
                            from: "module".to_string(),
                            to: al.to_string(),
                            rel_type: "imports".to_string(),
                            properties: props,
                        });
                    }
                }
                "dotted_name" => {
                    if let Ok(import_name) = child.utf8_text(source) {
                        let import_name = import_name.trim();
                        if let Some(ref fm) = from_module {
                            if fm == import_name {
                                continue;
                            }
                        }
                        let mut props = HashMap::new();
                        let module = from_module
                            .clone()
                            .unwrap_or_else(|| import_name.to_string());
                        props.insert("module".to_string(), module);

                        result.relations.push(Relation {
                            from: "module".to_string(),
                            to: import_name.to_string(),
                            rel_type: "imports".to_string(),
                            properties: props,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// REQ-AXO-902582 — extract dynamic imports from importlib / module_from_spec
    fn extract_dynamic_imports<'a>(
        &self,
        root: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
    ) {
        fn collect_strings<'b>(node: Node<'b>, source: &[u8], out: &mut Vec<String>) {
            if node.kind() == "string" {
                if let Ok(text) = node.utf8_text(source) {
                    let clean = text
                        .trim()
                        .trim_start_matches(|c| c == 'r' || c == 'b' || c == 'u' || c == 'f')
                        .trim_matches(|c| c == '"' || c == '\'');
                    out.push(clean.to_string());
                }
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_strings(child, source, out);
            }
        }

        let mut path_vars: HashMap<String, String> = HashMap::new();
        let mut spec_vars: HashMap<String, String> = HashMap::new();
        let mut assignments: Vec<(String, Node<'a>)> = Vec::new();

        let mut queue = vec![root];
        while let Some(current) = queue.pop() {
            if current.kind() == "assignment" {
                let left = current
                    .child_by_field_name("left")
                    .or_else(|| current.child(0));
                let right = current.child_by_field_name("right").or_else(|| {
                    let mut cursor = current.walk();
                    current.children(&mut cursor).last()
                });
                if let (Some(l), Some(r)) = (left, right) {
                    if let Ok(left_name) = l.utf8_text(source) {
                        assignments.push((left_name.trim().to_string(), r));
                    }
                }
            }
            let mut cursor = current.walk();
            for child in current.children(&mut cursor) {
                queue.push(child);
            }
        }

        // Phase 1: identify path variables and spec_from_file_location
        for (var_name, right_node) in &assignments {
            let right_text = right_node.utf8_text(source).unwrap_or("");
            let mut strings = Vec::new();
            collect_strings(*right_node, source, &mut strings);

            for s in &strings {
                if s.ends_with(".py") {
                    let file_name = std::path::Path::new(s)
                        .file_name()
                        .and_then(|os| os.to_str())
                        .unwrap_or(s);
                    path_vars.insert(var_name.clone(), file_name.to_string());
                }
            }

            if right_text.contains("spec_from_file_location") {
                let mut target_module = String::new();
                let mut file_hint = String::new();

                for s in &strings {
                    if s.ends_with(".py") {
                        file_hint = s.clone();
                    } else if target_module.is_empty() {
                        target_module = s.clone();
                    }
                }

                if file_hint.is_empty() {
                    for (pv, fname) in &path_vars {
                        if right_text.contains(pv) {
                            file_hint = fname.clone();
                            break;
                        }
                    }
                }

                let final_target = if !file_hint.is_empty() {
                    let clean = std::path::Path::new(&file_hint)
                        .file_name()
                        .and_then(|os| os.to_str())
                        .unwrap_or(&file_hint);
                    clean.strip_suffix(".py").unwrap_or(clean).to_string()
                } else {
                    target_module
                };

                if !final_target.is_empty() {
                    spec_vars.insert(var_name.clone(), final_target);
                }
            }
        }

        // Phase 2: identify module_from_spec or import_module
        for (var_name, right_node) in &assignments {
            let right_text = right_node.utf8_text(source).unwrap_or("");

            if right_text.contains("module_from_spec") {
                let mut target = None;
                for (spec_var, mod_target) in &spec_vars {
                    if right_text.contains(spec_var) {
                        target = Some(mod_target.clone());
                        break;
                    }
                }
                if target.is_none() && spec_vars.len() == 1 {
                    target = spec_vars.values().next().cloned();
                }

                if let Some(target_module) = target {
                    let mut props = HashMap::new();
                    props.insert("module".to_string(), target_module.clone());
                    props.insert("alias".to_string(), var_name.clone());
                    props.insert("dynamic".to_string(), "true".to_string());

                    result.relations.push(Relation {
                        from: "module".to_string(),
                        to: var_name.clone(),
                        rel_type: "imports".to_string(),
                        properties: props,
                    });
                }
            } else if right_text.contains("import_module") {
                let mut strings = Vec::new();
                collect_strings(*right_node, source, &mut strings);
                if let Some(mod_target) = strings.first() {
                    let mut props = HashMap::new();
                    props.insert("module".to_string(), mod_target.clone());
                    props.insert("alias".to_string(), var_name.clone());
                    props.insert("dynamic".to_string(), "true".to_string());

                    result.relations.push(Relation {
                        from: "module".to_string(),
                        to: var_name.clone(),
                        rel_type: "imports".to_string(),
                        properties: props,
                    });
                }
            }
        }
    }
}

impl Parser for PythonParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        if let Some(tree) = parse_with_wasm_safe("python", self.wasm_bytes, content) {
            let root = tree.root_node();
            let source = content.as_bytes();
            self.extract_dynamic_imports(root, source, &mut result);
            self.walk(root, source, &mut result, "");
        }

        // REQ-AXO-902330 — mark local target functions of framework_invokes relations
        // as entry points so that intra-file IoC callbacks (e.g. compute="_compute_x")
        // are recognized as active entry points at the parser layer.
        let mut invoked_targets: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for r in &result.relations {
            if r.rel_type == "framework_invokes" {
                invoked_targets.insert(r.to.clone());
                if let Some(leaf) = r.to.split('.').last() {
                    invoked_targets.insert(leaf.to_string());
                }
            }
        }

        if !invoked_targets.is_empty() {
            for sym in &mut result.symbols {
                let leaf = sym.name.split('.').last().unwrap_or(&sym.name);
                if invoked_targets.contains(&sym.name) || invoked_targets.contains(leaf) {
                    sym.is_entry_point = true;
                }
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    //! REQ-AXO-902185 (god-objects) — cyclomatic complexity regression tests,
    //! mirroring the Rust parser's coverage (parser/rust.rs) for the second
    //! language in the operator-chosen "all languages" scope.
    use super::*;
    use crate::parser::Parser;

    fn parser() -> PythonParser {
        PythonParser::new()
    }

    #[test]
    fn simple_function_has_complexity_one() {
        let p = parser();
        let result = p.parse("def f():\n    x = 1\n    y = x + 1\n");
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
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
        let p = parser();
        // 1 (base) + if + elif + while + for + except = 6.
        let result = p.parse(
            "def f(x):\n\
             \x20   if x > 0:\n\
             \x20       pass\n\
             \x20   elif x < 0:\n\
             \x20       pass\n\
             \x20   while x > 0:\n\
             \x20       break\n\
             \x20   for i in range(x):\n\
             \x20       pass\n\
             \x20   try:\n\
             \x20       pass\n\
             \x20   except ValueError:\n\
             \x20       pass\n",
        );
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        assert_eq!(
            f.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("6"),
            "props: {:?}",
            f.properties
        );
    }

    #[test]
    fn nested_function_branch_does_not_inflate_enclosing_complexity() {
        let p = parser();
        // outer: base 1 + 1 if = 2. The Python parser does not currently
        // extract nested `def` as its own Symbol (pre-existing gap, out of
        // scope here) — but `count_branches` must still exclude the nested
        // `function_definition` subtree, so its `if` does not leak into
        // outer's count.
        let result = p.parse(
            "def outer(x):\n\
             \x20   if x > 0:\n\
             \x20       pass\n\
             \x20   def inner(y):\n\
             \x20       if y > 0:\n\
             \x20           pass\n",
        );
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }
        let outer = result.symbols.iter().find(|s| s.name == "outer").unwrap();
        assert_eq!(
            outer
                .properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn python_importlib_dynamic_loading_and_aliased_calls_are_extracted() {
        // REQ-AXO-902582 — test importlib alias resolution and dynamic call extraction
        let p = parser();
        let code = r#"
import importlib.util
from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts" / "nexus-admission.py"
SPEC = importlib.util.spec_from_file_location("nexus_admission", MODULE_PATH)
NA = importlib.util.module_from_spec(SPEC)

def test_classify():
    NA.classify_pressure(100)
"#;
        let result = p.parse(code);
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }

        // Must extract imports relation linking alias NA to nexus-admission or nexus_admission
        let na_import = result
            .relations
            .iter()
            .find(|r| r.rel_type == "imports" && r.to == "NA");
        assert!(
            na_import.is_some(),
            "Expected imports relation for alias NA, got relations: {:?}",
            result.relations
        );
        let import_props = &na_import.unwrap().properties;
        assert!(
            import_props.get("module").is_some(),
            "Expected module property in NA import: {:?}",
            import_props
        );

        // Must extract call from test_classify to classify_pressure with receiver NA
        let call_rel = result
            .relations
            .iter()
            .find(|r| r.rel_type == "calls" && r.to == "classify_pressure");
        assert!(
            call_rel.is_some(),
            "Expected calls relation to classify_pressure, got relations: {:?}",
            result.relations
        );
        assert_eq!(
            call_rel
                .unwrap()
                .properties
                .get("receiver")
                .map(String::as_str),
            Some("NA")
        );
    }

    #[test]
    fn test_req_902423_python_contains_relations() {
        let p = parser();
        let code = r#"
class Agent:
    def execute(self):
        pass
"#;
        let result = p.parse(code);
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }
        let contains_agent_execute = result
            .relations
            .iter()
            .any(|r| r.rel_type == "contains" && r.from == "Agent" && r.to == "Agent.execute");
        assert!(
            contains_agent_execute,
            "Agent must contain Agent.execute: {:?}",
            result.relations
        );
    }

    #[test]
    fn test_req_902330_python_ioc_decorators() {
        let p = parser();
        let code = r#"
class AccountMove:
    @api.depends('line_ids.price_subtotal')
    def _compute_amount(self):
        pass

    @api.constrains('date')
    def _check_date(self):
        pass

    def regular_private_method(self):
        pass

@task
def background_worker():
    pass
"#;
        let result = p.parse(code);
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }

        let compute_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "AccountMove._compute_amount")
            .expect("AccountMove._compute_amount must be parsed");
        assert!(
            compute_sym.is_entry_point,
            "@api.depends method must be an entry point"
        );
        assert_eq!(
            compute_sym
                .properties
                .get("framework_ioc")
                .map(String::as_str),
            Some("true")
        );

        let constrains_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "AccountMove._check_date")
            .expect("AccountMove._check_date must be parsed");
        assert!(
            constrains_sym.is_entry_point,
            "@api.constrains method must be an entry point"
        );

        let regular_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "AccountMove.regular_private_method")
            .expect("AccountMove.regular_private_method must be parsed");
        assert!(
            !regular_sym.is_entry_point,
            "Unannotated private method must not be an entry point"
        );

        let worker_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "background_worker")
            .expect("background_worker must be parsed");
        assert!(
            worker_sym.is_entry_point,
            "@task function must be an entry point"
        );
    }

    #[test]
    fn test_req_902330_python_framework_keyword_args() {
        let p = parser();
        let code = r#"
class SaleOrder:
    amount = fields.Monetary(compute='_compute_amount', inverse='_inverse_amount')

    def _compute_amount(self):
        pass

    def _inverse_amount(self):
        pass
"#;
        let result = p.parse(code);
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }

        let compute_rel = result
            .relations
            .iter()
            .find(|r| r.rel_type == "framework_invokes" && r.to == "SaleOrder._compute_amount");
        assert!(
            compute_rel.is_some(),
            "compute='_compute_amount' must emit framework_invokes relation: {:?}",
            result.relations
        );

        let inverse_rel = result
            .relations
            .iter()
            .find(|r| r.rel_type == "framework_invokes" && r.to == "SaleOrder._inverse_amount");
        assert!(
            inverse_rel.is_some(),
            "inverse='_inverse_amount' must emit framework_invokes relation: {:?}",
            result.relations
        );

        let compute_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "SaleOrder._compute_amount")
            .expect("SaleOrder._compute_amount must be parsed");
        assert!(
            compute_sym.is_entry_point,
            "Target of compute= must be marked as entry point"
        );

        let inverse_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "SaleOrder._inverse_amount")
            .expect("SaleOrder._inverse_amount must be parsed");
        assert!(
            inverse_sym.is_entry_point,
            "Target of inverse= must be marked as entry point"
        );
    }

    #[test]
    fn req_902660_python_tests_and_testcases_marked_as_tested() {
        let p = PythonParser::new();
        let code = r#"
import unittest

class CalculationTestCase(unittest.TestCase):
    def run_calculation_test(self):
        assert 1 == 1

def helper_function_test():
    pass
"#;
        let result = p.parse(code);
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }

        let cls = result
            .symbols
            .iter()
            .find(|s| s.name == "CalculationTestCase")
            .expect("Class must exist");
        assert!(cls.tested, "TestCase class must be tested=true");

        let method = result
            .symbols
            .iter()
            .find(|s| s.name == "CalculationTestCase.run_calculation_test")
            .expect("Method must exist");
        assert!(
            method.tested,
            "Method inside TestCase or ending in _test must be tested=true"
        );

        let func = result
            .symbols
            .iter()
            .find(|s| s.name == "helper_function_test")
            .expect("Function must exist");
        assert!(func.tested, "Function ending in _test must be tested=true");
    }

    #[test]
    fn req_902663_python_sqlalchemy_models_and_relationships() {
        let p = PythonParser::new();
        let code = r#"
class User(Base):
    __tablename__ = "users"

    id = Column(Integer, primary_key=True)
    org_id = Column(Integer, ForeignKey("organizations.id"))
    organization = relationship("Organization", back_populates="users")
    posts = relationship("Post", back_populates="author")
"#;
        let result = p.parse(code);
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }

        let user_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "User")
            .expect("User class must exist");
        assert_eq!(
            user_sym.properties.get("table").map(|s| s.as_str()),
            Some("users")
        );

        // References to table "users"
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "User" && r.to == "users" && r.rel_type == "references"));

        // ForeignKey references "organizations"
        assert!(result.relations.iter().any(|r| r.from == "User"
            && r.to == "organizations"
            && r.rel_type == "references"
            && r.properties.get("foreign_key") == Some(&"organizations.id".to_string())));

        // Relationships reference "Organization" and "Post"
        assert!(result.relations.iter().any(|r| r.from == "User"
            && r.to == "Organization"
            && r.rel_type == "references"
            && r.properties.get("relation") == Some(&"relationship".to_string())));
        assert!(result.relations.iter().any(|r| r.from == "User"
            && r.to == "Post"
            && r.rel_type == "references"
            && r.properties.get("relation") == Some(&"relationship".to_string())));

        // Field symbols
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "User.id" && s.kind == "field"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "User.org_id" && s.kind == "field"));
    }

    #[test]
    fn test_tranche7_python_async_workers_and_messaging() {
        let p = PythonParser::new();
        let code = r#"
from celery import Celery
from fastapi import FastAPI, WebSocket

app = FastAPI()

@app.task(queue="orders_queue")
def process_order(order_id):
    return order_id

@app.websocket("/ws/market_feed")
async def websocket_endpoint(websocket: WebSocket):
    await websocket.accept()

async def send_events(producer):
    await producer.send_and_wait("orders.completed", b"payload")
    process_order.delay(42)
"#;
        let result = p.parse(code);
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }

        let worker = result
            .symbols
            .iter()
            .find(|s| s.name == "process_order" && s.kind == "worker");
        assert!(worker.is_some(), "process_order should be marked as worker");
        assert_eq!(
            worker.unwrap().properties.get("queue").map(|s| s.as_str()),
            Some("orders_queue")
        );

        let ws = result
            .symbols
            .iter()
            .find(|s| s.kind == "websocket_endpoint");
        assert!(ws.is_some(), "websocket endpoint should be found");
        assert_eq!(
            ws.unwrap().properties.get("route").map(|s| s.as_str()),
            Some("/ws/market_feed")
        );

        let topic = result
            .symbols
            .iter()
            .find(|s| s.kind == "topic" && s.name == "orders.completed");
        assert!(
            topic.is_some(),
            "topic orders.completed should be extracted"
        );

        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "publishes_to" && r.to == "orders.completed"),
            "should publish to orders.completed"
        );
        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "dispatches_job" && r.to == "process_order"),
            "should dispatch job process_order"
        );
    }
}
