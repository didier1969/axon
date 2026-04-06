use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct SollRestoreCounts {
    pub vision: usize,
    pub pillars: usize,
    pub concepts: usize,
    pub milestones: usize,
    pub requirements: usize,
    pub decisions: usize,
    pub validations: usize,
    pub guidelines: usize,
    pub relations: usize,
}

#[derive(Default)]
pub(crate) struct ParsedSollExport {
    pub vision: Vec<ParsedVision>,
    pub pillars: Vec<ParsedPillar>,
    pub concepts: Vec<ParsedConcept>,
    pub milestones: Vec<ParsedMilestone>,
    pub requirements: Vec<ParsedRequirement>,
    pub decisions: Vec<ParsedDecision>,
    pub validations: Vec<ParsedValidation>,
    pub relations: Vec<ParsedRelation>,
    pub guidelines: Vec<ParsedGuideline>,
}


pub(crate) struct ParsedGuideline {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub metadata: Option<String>,
}

pub(crate) struct ParsedVision {
    pub title: String,
    pub description: String,
    pub goal: String,
    pub metadata: Option<String>,
}

pub(crate) struct ParsedPillar {
    pub id: String,
    pub title: String,
    pub description: String,
    pub metadata: Option<String>,
}

pub(crate) struct ParsedConcept {
    pub id: String,
    pub name: String,
    pub explanation: String,
    pub rationale: String,
    pub metadata: Option<String>,
}

pub(crate) struct ParsedMilestone {
    pub id: String,
    pub title: String,
    pub status: String,
    pub metadata: Option<String>,
}

pub(crate) struct ParsedRequirement {
    pub id: String,
    pub title: String,
    pub priority: String,
    pub description: String,
    pub status: Option<String>,
    pub metadata: Option<String>,
}

#[allow(dead_code)]
pub(crate) struct ParsedDecision {
    pub id: String,
    pub title: String,
    pub status: String,
    pub description: Option<String>,
    pub context: Option<String>,
    pub rationale: String,
    pub metadata: Option<String>,
}

#[allow(dead_code)]
pub(crate) struct ParsedValidation {
    pub id: String,
    pub result: String,
    pub method: String,
    pub timestamp: i64,
    pub metadata: Option<String>,
}

pub(crate) struct ParsedRelation {
    pub relation_type: String,
    pub source_id: String,
    pub target_id: String,
}

fn is_repo_root(path: &Path) -> bool {
    path.join("README.md").is_file()
        && path.join("docs").is_dir()
        && path.join("src/axon-core/Cargo.toml").is_file()
}

pub(crate) fn resolve_repo_root_from(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|ancestor| is_repo_root(ancestor))
        .map(Path::to_path_buf)
}

pub(crate) fn resolve_repo_root() -> Option<PathBuf> {
    if let Ok(current_dir) = std::env::current_dir() {
        if let Some(repo_root) = resolve_repo_root_from(&current_dir) {
            return Some(repo_root);
        }
    }

    resolve_repo_root_from(Path::new(env!("CARGO_MANIFEST_DIR")))
}

pub(crate) fn canonical_soll_export_dir() -> Option<PathBuf> {
    resolve_repo_root().map(|root| root.join("docs/vision"))
}

pub(crate) fn find_latest_soll_export() -> Option<String> {
    let export_dir = canonical_soll_export_dir()?;
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(export_dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("SOLL_EXPORT_") && name.ends_with(".md"))
                .unwrap_or(false)
        })
        .collect();

    candidates.sort();
    candidates.last().map(|p| p.to_string_lossy().to_string())
}

pub(crate) fn parse_soll_export(markdown: &str) -> std::result::Result<ParsedSollExport, String> {
    let mut parsed = ParsedSollExport::default();
    let mut lines = markdown.lines().peekable();
    let mut current_type = String::new();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("## Entités : ") {
            current_type = trimmed.strip_prefix("## Entités : ").unwrap_or("").trim().to_string();
            continue;
        }

        if trimmed.starts_with("## Topologie") {
            current_type = "Topology".to_string();
            continue;
        }

        if current_type == "Topology" && trimmed.contains(" -- ") && trimmed.contains(" --> ") {
            // "  source -- relation --> target;"
            let parts: Vec<&str> = trimmed.split(" -- ").collect();
            if parts.len() == 2 {
                let source_id = parts[0].trim().to_string();
                let right_parts: Vec<&str> = parts[1].split(" --> ").collect();
                if right_parts.len() == 2 {
                    let relation_type = right_parts[0].trim().to_string();
                    let target_id = right_parts[1].trim_end_matches(';').trim().to_string();
                    parsed.relations.push(ParsedRelation {
                        relation_type,
                        source_id,
                        target_id,
                    });
                }
            }
            continue;
        }

        if trimmed.starts_with("### ") {
            let raw = trimmed.strip_prefix("### ").unwrap_or("").trim();
            let (id, title) = if let Some((id_str, title_str)) = raw.split_once(" - ") {
                (id_str.trim().to_string(), title_str.trim().to_string())
            } else if let Some(id_str) = raw.strip_suffix(" -") {
                (id_str.trim().to_string(), "".to_string())
            } else {
                (raw.to_string(), "".to_string())
            };

            let mut description = String::new();
            let mut status = String::new();
            let mut metadata = None;

            while let Some(next) = lines.peek() {
                let next_trim = next.trim();
                if next_trim.starts_with("### ") || next_trim.starts_with("## ") {
                    break;
                }
                
                if next_trim.starts_with("**Description:**") {
                    description = next_trim.strip_prefix("**Description:**").unwrap_or("").trim().to_string();
                } else if next_trim.starts_with("**Status:**") {
                    status = next_trim.strip_prefix("**Status:**").unwrap_or("").trim().to_string();
                } else if next_trim.starts_with("**Meta:**") {
                    let meta_str = next_trim.strip_prefix("**Meta:**").unwrap_or("").trim().trim_matches('`').to_string();
                    metadata = Some(meta_str);
                }
                lines.next();
            }

            match current_type.as_str() {
                "Vision" => parsed.vision.push(ParsedVision { title, description, goal: "".to_string(), metadata }),
                "Pillar" => parsed.pillars.push(ParsedPillar { id, title, description, metadata }),
                "Requirement" => parsed.requirements.push(ParsedRequirement { id, title, priority: "".to_string(), description, status: Some(status), metadata }),
                "Concept" => parsed.concepts.push(ParsedConcept { id, name: title, explanation: description, rationale: "".to_string(), metadata }),
                "Decision" => parsed.decisions.push(ParsedDecision { id, title, status, description: Some(description), context: None, rationale: "".to_string(), metadata }),
                "Milestone" => parsed.milestones.push(ParsedMilestone { id, title, status, metadata }),
                "Validation" => parsed.validations.push(ParsedValidation { id, result: status, method: "".to_string(), timestamp: 0, metadata }),
                "Guideline" => parsed.guidelines.push(ParsedGuideline { id, title, description, status, metadata }),
                _ => {}
            }
        }
    }

    if parsed.vision.is_empty()
        && parsed.pillars.is_empty()
        && parsed.concepts.is_empty()
        && parsed.milestones.is_empty()
        && parsed.requirements.is_empty()
        && parsed.decisions.is_empty()
        && parsed.validations.is_empty()
        && parsed.relations.is_empty()
        && parsed.guidelines.is_empty()
    {
        return Err("no restorable SOLL entities found in export".to_string());
    }

    Ok(parsed)
}
#[allow(dead_code)]
fn parse_bold_bullet(line: &str) -> Option<(String, String)> {
    let rest = line.trim_start_matches('*').trim();
    let rest = rest.strip_prefix("**")?;
    let end = rest.find("**")?;
    let id = rest[..end].trim().to_string();
    let tail = rest[end + 2..].trim();
    let tail = tail.strip_prefix(':').unwrap_or(tail).trim().to_string();
    Some((id, tail))
}

#[allow(dead_code)]
fn split_title_paren(raw: String) -> (String, String) {
    if let Some(open) = raw.rfind(" (") {
        if raw.ends_with(')') {
            let title = raw[..open].trim().to_string();
            let desc = raw[open + 2..raw.len() - 1].trim().to_string();
            return (title, desc);
        }
    }
    (raw, String::new())
}

#[allow(dead_code)]
fn parse_validation_line(line: &str) -> Option<ParsedValidation> {
    let trimmed = line.trim().trim_start_matches('*').trim();
    let after_first_tick = trimmed.strip_prefix('`')?;
    let id_end = after_first_tick.find('`')?;
    let id = after_first_tick[..id_end].to_string();
    let after_id = after_first_tick[id_end + 1..].trim();
    let after_colon = after_id.strip_prefix(':')?.trim();
    let after_result_open = after_colon.strip_prefix("**")?;
    let result_end = after_result_open.find("**")?;
    let result = after_result_open[..result_end].to_string();
    let after_result = after_result_open[result_end + 2..].trim();
    let method_start = after_result.find('`')?;
    let after_method_tick = &after_result[method_start + 1..];
    let method_end = after_method_tick.find('`')?;
    let method = after_method_tick[..method_end].to_string();
    let timestamp = after_result
        .rsplit(' ')
        .next()
        .and_then(|s| s.trim_end_matches(')').parse::<i64>().ok())
        .unwrap_or(0);

    Some(ParsedValidation {
        id,
        result,
        method,
        timestamp,
        metadata: None,
    })
}

#[allow(dead_code)]
fn parse_relation_line(line: &str) -> Option<ParsedRelation> {
    let trimmed = line.trim().trim_start_matches('*').trim();
    let after_kind_tick = trimmed.strip_prefix('`')?;
    let kind_end = after_kind_tick.find('`')?;
    let relation_type = after_kind_tick[..kind_end].trim().to_string();
    let after_kind = after_kind_tick[kind_end + 1..].trim();
    let after_colon = after_kind.strip_prefix(':')?.trim();
    let after_source_tick = after_colon.strip_prefix('`')?;
    let source_end = after_source_tick.find('`')?;
    let source_id = after_source_tick[..source_end].trim().to_string();
    let after_source = after_source_tick[source_end + 1..].trim();
    let after_arrow = after_source.strip_prefix("->")?.trim();
    let after_target_tick = after_arrow.strip_prefix('`')?;
    let target_end = after_target_tick.find('`')?;
    let target_id = after_target_tick[..target_end].trim().to_string();

    Some(ParsedRelation {
        relation_type,
        source_id,
        target_id,
    })
}

#[allow(dead_code)]
fn parse_optional_metadata_line<'a, I>(
    lines: &mut std::iter::Peekable<I>,
    prefix: &str,
) -> Option<String>
where
    I: Iterator<Item = &'a str>,
{
    parse_optional_line(lines, prefix).map(|s| s.trim_matches('`').to_string())
}

#[allow(dead_code)]
fn parse_optional_backticked_line<'a, I>(
    lines: &mut std::iter::Peekable<I>,
    prefix: &str,
) -> Option<String>
where
    I: Iterator<Item = &'a str>,
{
    parse_optional_line(lines, prefix).map(|s| s.trim_matches('`').to_string())
}

#[allow(dead_code)]
fn parse_optional_plain_line<'a, I>(
    lines: &mut std::iter::Peekable<I>,
    prefix: &str,
) -> Option<String>
where
    I: Iterator<Item = &'a str>,
{
    parse_optional_line(lines, prefix)
}

#[allow(dead_code)]
fn parse_optional_line<'a, I>(lines: &mut std::iter::Peekable<I>, prefix: &str) -> Option<String>
where
    I: Iterator<Item = &'a str>,
{
    while matches!(lines.peek(), Some(next) if next.trim().is_empty()) {
        lines.next();
    }

    let next = lines.peek()?.trim();
    let stripped = next
        .strip_prefix(prefix)
        .or_else(|| next.strip_prefix(&format!("{} ", prefix)))?
        .trim()
        .to_string();
    lines.next();
    Some(stripped)
}
