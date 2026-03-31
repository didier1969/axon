use std::path::PathBuf;

#[derive(Default)]
pub(crate) struct SollRestoreCounts {
    pub vision: usize,
    pub pillars: usize,
    pub concepts: usize,
    pub milestones: usize,
    pub requirements: usize,
    pub decisions: usize,
    pub validations: usize,
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

pub(crate) struct ParsedDecision {
    pub id: String,
    pub title: String,
    pub status: String,
    pub description: Option<String>,
    pub context: Option<String>,
    pub rationale: String,
    pub metadata: Option<String>,
}

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

pub(crate) fn find_latest_soll_export() -> Option<String> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir("docs/vision")
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
    let mut section = "";

    while let Some(line) = lines.next() {
        let trimmed = line.trim();

        if trimmed.starts_with("## ") {
            section = trimmed;
            continue;
        }

        match section {
            "## 1. Vision & Objectifs Stratégiques" if trimmed.starts_with("### ") => {
                let title = trimmed.trim_start_matches("### ").trim().to_string();
                let description = lines.next().unwrap_or("").trim().trim_start_matches("**Description:**").trim().to_string();
                let goal = lines.next().unwrap_or("").trim().trim_start_matches("**Goal:**").trim().to_string();
                let metadata_line = lines.next().unwrap_or("").trim();
                let metadata = metadata_line
                    .strip_prefix("**Meta:**")
                    .map(|s| s.trim().trim_matches('`').to_string());
                parsed.vision.push(ParsedVision { title, description, goal, metadata });
            }
            "## 2. Piliers d'Architecture" if trimmed.starts_with("* **") => {
                if let Some((id, rest)) = parse_bold_bullet(trimmed) {
                    let (title, description) = split_title_paren(rest);
                    let metadata = parse_optional_metadata_line(&mut lines, "Meta:");
                    parsed.pillars.push(ParsedPillar { id, title, description, metadata });
                }
            }
            "## 2b. Concepts" if trimmed.starts_with("* **") => {
                if let Some((name, rest)) = parse_bold_bullet(trimmed) {
                    let (explanation, rationale) = split_title_paren(rest);
                    let metadata = parse_optional_metadata_line(&mut lines, "Meta:");
                    parsed.concepts.push(ParsedConcept { name, explanation, rationale, metadata });
                }
            }
            "## 3. Jalons & Roadmap (Milestones)" if trimmed.starts_with("### ") => {
                let raw = trimmed.trim_start_matches("### ").trim();
                let mut parts = raw.splitn(2, " : ");
                let id = parts.next().unwrap_or("").trim().to_string();
                let title = parts.next().unwrap_or("").trim().to_string();
                let status_line = lines.next().unwrap_or("").trim();
                let status = status_line
                    .trim_start_matches("*Statut :*")
                    .trim()
                    .trim_matches('`')
                    .to_string();
                let metadata = parse_optional_metadata_line(&mut lines, "*Meta :*");
                parsed.milestones.push(ParsedMilestone { id, title, status, metadata });
            }
            "## 4. Exigences & Rayon d'Impact (Requirements)" if trimmed.starts_with("### ") => {
                let raw = trimmed.trim_start_matches("### ").trim();
                let mut parts = raw.splitn(2, " - ");
                let id = parts.next().unwrap_or("").trim().to_string();
                let title = parts.next().unwrap_or("").trim().to_string();
                let priority_line = lines.next().unwrap_or("").trim();
                let description_line = lines.next().unwrap_or("").trim();
                let priority = priority_line
                    .trim_start_matches("*Priorité :*")
                    .trim()
                    .trim_matches('`')
                    .to_string();
                let description = description_line
                    .trim_start_matches("*Description :*")
                    .trim()
                    .to_string();
                let status = parse_optional_backticked_line(&mut lines, "*Statut :*");
                let metadata = parse_optional_metadata_line(&mut lines, "*Meta :*");
                parsed.requirements.push(ParsedRequirement { id, title, priority, description, status, metadata });
            }
            "## 5. Registre des Décisions (ADR)" if trimmed.starts_with("### ") => {
                let id = trimmed.trim_start_matches("### ").trim().to_string();
                let title = lines.next().unwrap_or("").trim().trim_start_matches("**Titre :**").trim().to_string();
                let status = lines.next().unwrap_or("").trim().trim_start_matches("**Statut :**").trim().trim_matches('`').to_string();
                let context = parse_optional_plain_line(&mut lines, "**Contexte :**");
                let description = parse_optional_plain_line(&mut lines, "**Description :**");
                let rationale = lines.next().unwrap_or("").trim().trim_start_matches("**Rationnel :**").trim().to_string();
                let metadata = parse_optional_metadata_line(&mut lines, "**Meta :**");
                parsed.decisions.push(ParsedDecision { id, title, status, description, context, rationale, metadata });
            }
            "## 6. Preuves de Validation & Witness" if trimmed.starts_with('*') => {
                if let Some(validation) = parse_validation_line(trimmed) {
                    let metadata = parse_optional_metadata_line(&mut lines, "Meta:");
                    parsed.validations.push(ParsedValidation { metadata, ..validation });
                }
            }
            "## 7. Liens de Traçabilité SOLL" if trimmed.starts_with('*') => {
                if let Some(relation) = parse_relation_line(trimmed) {
                    parsed.relations.push(relation);
                }
            }
            _ => {}
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
    {
        return Err("no restorable SOLL entities found in export".to_string());
    }

    Ok(parsed)
}

fn parse_bold_bullet(line: &str) -> Option<(String, String)> {
    let rest = line.trim_start_matches('*').trim();
    let rest = rest.strip_prefix("**")?;
    let end = rest.find("**")?;
    let id = rest[..end].trim().to_string();
    let tail = rest[end + 2..].trim();
    let tail = tail.strip_prefix(':').unwrap_or(tail).trim().to_string();
    Some((id, tail))
}

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

    Some(ParsedValidation { id, result, method, timestamp, metadata: None })
}

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

    Some(ParsedRelation { relation_type, source_id, target_id })
}

fn parse_optional_metadata_line<'a, I>(
    lines: &mut std::iter::Peekable<I>,
    prefix: &str,
) -> Option<String>
where
    I: Iterator<Item = &'a str>,
{
    parse_optional_line(lines, prefix).map(|s| s.trim_matches('`').to_string())
}

fn parse_optional_backticked_line<'a, I>(
    lines: &mut std::iter::Peekable<I>,
    prefix: &str,
) -> Option<String>
where
    I: Iterator<Item = &'a str>,
{
    parse_optional_line(lines, prefix).map(|s| s.trim_matches('`').to_string())
}

fn parse_optional_plain_line<'a, I>(
    lines: &mut std::iter::Peekable<I>,
    prefix: &str,
) -> Option<String>
where
    I: Iterator<Item = &'a str>,
{
    parse_optional_line(lines, prefix)
}

fn parse_optional_line<'a, I>(
    lines: &mut std::iter::Peekable<I>,
    prefix: &str,
) -> Option<String>
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
