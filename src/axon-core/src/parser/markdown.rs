use super::{ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::{Language, Node, Parser as TSParser};

pub struct MarkdownParser {
    language: Language,
}

impl Default for MarkdownParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownParser {
    pub fn new() -> Self {
        Self {
            language: tree_sitter_md::LANGUAGE.into(),
        }
    }

    fn collect_headings<'a>(&self, node: Node<'a>, source: &[u8], headings: &mut Vec<(usize, usize, String)>) {
        let kind = node.kind();
        if kind == "atx_heading" {
            let mut level = 0;
            let mut name = String::new();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                let child_kind = child.kind();
                if child_kind.starts_with("atx_h") && child_kind.ends_with("_marker") {
                    level = child.utf8_text(source).unwrap_or("").trim().len();
                } else if child_kind == "inline" {
                    name = child.utf8_text(source).unwrap_or("").trim().to_string();
                }
            }
            if !name.is_empty() {
                headings.push((node.start_position().row + 1, level, name));
            }
            return;
        }
        
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_headings(child, source, headings);
        }
    }

    fn extract_frontmatter(&self, lines: &[&str], symbols: &mut Vec<Symbol>) -> usize {
        if lines.is_empty() || lines[0].trim() != "---" {
            return 0;
        }
        let mut end_idx = 0;
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.trim() == "---" {
                end_idx = i;
                break;
            }
        }
        if end_idx == 0 {
            return 0;
        }
        for (i, line) in lines[1..end_idx].iter().enumerate() {
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim();
                if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') {
                    symbols.push(Symbol {
                        name: format!("frontmatter:{}", key),
                        kind: "function".to_string(),
                        start_line: i + 2,
                        end_line: i + 2,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        properties: HashMap::new(),
                    
                        embedding: None,
                    });
                }
            }
        }
        end_idx + 1
    }

    fn is_table_line(&self, line: &str) -> bool {
        let trimmed = line.trim();
        trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() > 1
    }

    fn extract_tables(&self, lines: &[&str], symbols: &mut Vec<Symbol>) {
        let mut i = 0;
        while i < lines.len() {
            if self.is_table_line(lines[i]) {
                let table_start = i;
                let mut j = i + 1;
                while j < lines.len() && self.is_table_line(lines[j]) {
                    j += 1;
                }
                if j - table_start >= 2 {
                    let header = lines[table_start];
                    let cells: Vec<&str> = header.split('|').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
                    let first_header = if cells.is_empty() { "table" } else { cells[0] };
                    
                    symbols.push(Symbol {
                        name: format!("table:{}", first_header),
                        kind: "section".to_string(),
                        start_line: table_start + 1,
                        end_line: j,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        properties: HashMap::new(),
                    
                        embedding: None,
                    });
                    i = j;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
    }

    fn is_fence(&self, line: &str) -> Option<String> {
        let trimmed = line.trim_start();
        trimmed.strip_prefix("```").map(|stripped| stripped.trim().to_string())
    }

    fn extract_links(&self, line: &str, relations: &mut Vec<Relation>) {
        let mut i = 0;
        while let Some(start_bracket) = line[i..].find('[') {
            let rel_start = i + start_bracket;
            if let Some(end_bracket) = line[rel_start..].find(']') {
                let rel_end = rel_start + end_bracket;
                if line[rel_end..].starts_with("](") {
                    if let Some(end_paren) = line[rel_end + 2..].find(')') {
                        let url_start = rel_end + 2;
                        let url_end = url_start + end_paren;
                        let url = &line[url_start..url_end];
                        relations.push(Relation {
                            from: "".to_string(),
                            to: url.to_string(),
                            rel_type: "imports".to_string(),
                            properties: HashMap::new(),
                        });
                        i = url_end + 1;
                        continue;
                    }
                }
            }
            i = rel_start + 1;
        }
    }

    fn extract_links_and_fences(&self, lines: &[&str], symbols: &mut Vec<Symbol>, relations: &mut Vec<Relation>) {
        let mut in_code_block = false;
        let mut lang_tag = String::new();
        let mut block_start = 0;

        for (i, &line) in lines.iter().enumerate() {
            let line_no = i + 1;
            if let Some(tag) = self.is_fence(line) {
                if !in_code_block {
                    in_code_block = true;
                    lang_tag = tag;
                    block_start = line_no;
                    if !lang_tag.is_empty() {
                        relations.push(Relation {
                            from: "".to_string(),
                            to: lang_tag.clone(),
                            rel_type: "calls".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                } else {
                    in_code_block = false;
                    if ["mermaid", "plantuml", "dot"].contains(&lang_tag.as_str()) {
                        let mut props = HashMap::new();
                        props.insert("diagram".to_string(), "true".to_string());
                        symbols.push(Symbol {
                            name: format!("diagram:{}", lang_tag),
                            kind: "section".to_string(),
                            start_line: block_start,
                            end_line: line_no,
                            docstring: None,
                            is_entry_point: false,
                        is_public: true,
                            properties: props,
                        
                            embedding: None,
                        });
                    }
                }
                continue;
            }

            if !in_code_block {
                self.extract_links(line, relations);
            }
        }
    }
}

impl Parser for MarkdownParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut parser = TSParser::new();
        parser.set_language(&self.language).unwrap();
        let tree = parser.parse(content, None).unwrap();

        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let lines: Vec<&str> = content.lines().collect();

        // Pass 1: Frontmatter
        self.extract_frontmatter(&lines, &mut symbols);

        // Pass 2: Headings via tree-sitter
        let mut headings = Vec::new();
        self.collect_headings(tree.root_node(), content.as_bytes(), &mut headings);
        
        let total_lines = lines.len();
        for (idx, &(start_line, level, ref name)) in headings.iter().enumerate() {
            let end_line = if idx + 1 < headings.len() {
                headings[idx + 1].0 - 1
            } else {
                total_lines
            };

            symbols.push(Symbol {
                name: name.clone(),
                kind: "section".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: level == 1,
                        is_public: true,
                properties: HashMap::new(),
            
                embedding: None,
            });
        }

        // Pass 3: Tables
        self.extract_tables(&lines, &mut symbols);

        // Pass 4: Links and code fences
        self.extract_links_and_fences(&lines, &mut symbols, &mut relations);

        ExtractionResult { symbols, relations }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_markdown() {
        let code = r#"---
title: Hello World
author: John Doe
---

# Main Title

This is a link: [OpenAI](https://openai.com).

## Subtitle

Here is some code:

```rust
fn main() {
    println!("Hello");
}
```

```mermaid
graph TD;
    A-->B;
```

| Header 1 | Header 2 |
| -------- | -------- |
| Cell 1   | Cell 2   |
"#;
        let parser = MarkdownParser::new();
        let result = parser.parse(code);

        assert!(result.symbols.iter().any(|s| s.name == "frontmatter:title" && s.kind == "function"));
        assert!(result.symbols.iter().any(|s| s.name == "Main Title" && s.kind == "section" && s.is_entry_point));
        assert!(result.symbols.iter().any(|s| s.name == "Subtitle" && s.kind == "section"));
        
        assert!(result.relations.iter().any(|r| r.to == "rust" && r.rel_type == "calls"));
        assert!(result.relations.iter().any(|r| r.to == "https://openai.com" && r.rel_type == "imports"));

        assert!(result.symbols.iter().any(|s| s.name == "diagram:mermaid" && s.kind == "section"));
        assert!(result.symbols.iter().any(|s| s.name == "table:Header 1" && s.kind == "section"));
    }
}