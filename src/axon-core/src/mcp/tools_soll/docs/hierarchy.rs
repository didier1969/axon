use super::*;

pub(super) fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(super) fn entity_type_short_label(entity_type: &str) -> &str {
    match entity_type {
        "Portfolio" => "GLO",
        "Project" => "PRJ",
        "Vision" => "VIS",
        "Pillar" => "PIL",
        "Requirement" => "REQ",
        "Decision" => "DEC",
        "Concept" => "CPT",
        "Guideline" => "GUI",
        "Milestone" => "MIL",
        "Validation" => "VAL",
        "Stakeholder" => "STK",
        _ => entity_type,
    }
}

pub(super) fn edge_key(edge: &SollDocEdge) -> String {
    format!(
        "{}--{}-->{}",
        edge.source_id, edge.relation_type, edge.target_id
    )
}

pub(super) fn node_file_name(node_id: &str) -> String {
    format!("{}.html", node_id)
}

pub(super) fn subtree_file_name(node_id: &str) -> String {
    format!("{}.html", node_id)
}

fn entity_type_to_kind(entity_type: &str) -> Option<&'static str> {
    match entity_type {
        "Vision" => Some("VIS"),
        "Pillar" => Some("PIL"),
        "Requirement" => Some("REQ"),
        "Decision" => Some("DEC"),
        "Concept" => Some("CPT"),
        "Guideline" => Some("GUI"),
        "Milestone" => Some("MIL"),
        "Validation" => Some("VAL"),
        "Stakeholder" => Some("STK"),
        _ => None,
    }
}

fn projection_child_types(parent_type: &str) -> Vec<&'static str> {
    let Some(parent_kind) = entity_type_to_kind(parent_type) else {
        return Vec::new();
    };
    let mut children = SOLL_RELATION_ENDPOINT_KINDS
        .iter()
        .filter_map(|source_kind| {
            let policy = relation_policy_for_pair(source_kind, parent_kind)?;
            if !matches!(policy.projection.role, ProjectionRole::Primary) {
                return None;
            }
            let source_projection = kind_projection_policy(source_kind)?;
            if !source_projection.breadcrumb_eligible {
                return None;
            }
            let child_type: &'static str = match source_kind.as_ref() {
                "VIS" => "Vision",
                "PIL" => "Pillar",
                "REQ" => "Requirement",
                "DEC" => "Decision",
                "CPT" => "Concept",
                "GUI" => "Guideline",
                "MIL" => "Milestone",
                "VAL" => "Validation",
                "STK" => "Stakeholder",
                _ => return None,
            };
            Some((policy.projection.child_order_rank, child_type))
        })
        .collect::<Vec<_>>();
    children.sort_by(|left, right| left.cmp(right));
    children
        .into_iter()
        .map(|(_, child_type)| child_type)
        .collect()
}

fn hierarchy_child_types(parent_type: &str) -> &'static [&'static str] {
    match parent_type {
        "Project" => &["Vision"],
        "Vision" => &["Pillar"],
        "Pillar" => &["Requirement"],
        "Requirement" => &[
            "Decision",
            "Validation",
            "Guideline",
            "Concept",
            "Milestone",
            "Stakeholder",
        ],
        _ => &[],
    }
}

fn hierarchy_relation_allowed(parent_type: &str, child_type: &str) -> bool {
    let canonical = projection_child_types(parent_type);
    if !canonical.is_empty() {
        return canonical.iter().any(|candidate| *candidate == child_type);
    }
    hierarchy_child_types(parent_type)
        .iter()
        .any(|candidate| *candidate == child_type)
}

fn entity_type_sort_rank(entity_type: &str) -> usize {
    if let Some(kind) = entity_type_to_kind(entity_type) {
        if let Some(policy) = kind_projection_policy(kind) {
            return policy.tree_order_rank;
        }
    }
    match entity_type {
        "Project" => 0,
        "Vision" => 1,
        "Pillar" => 2,
        "Requirement" => 3,
        "Decision" => 4,
        "Validation" => 5,
        "Guideline" => 6,
        "Concept" => 7,
        "Milestone" => 8,
        "Stakeholder" => 9,
        _ => 99,
    }
}

fn preferred_parent_sort_key(node: &SollDocNode) -> (usize, &str, &str) {
    (
        entity_type_sort_rank(&node.entity_type),
        node.id.as_str(),
        node.title.as_str(),
    )
}

pub(super) fn hierarchy_candidate_parent_ids(
    node_id: &str,
    outgoing: &HashMap<String, Vec<SollDocEdge>>,
    nodes_by_id: &HashMap<String, SollDocNode>,
) -> Vec<String> {
    let Some(node) = nodes_by_id.get(node_id) else {
        return Vec::new();
    };
    let mut parent_ids = outgoing
        .get(node_id)
        .into_iter()
        .flatten()
        .filter_map(|edge| {
            let candidate = nodes_by_id.get(&edge.target_id)?;
            if hierarchy_relation_allowed(&candidate.entity_type, &node.entity_type) {
                let pair_projection = entity_type_to_kind(&node.entity_type)
                    .zip(entity_type_to_kind(&candidate.entity_type))
                    .and_then(|(child_kind, parent_kind)| {
                        relation_policy_for_pair(child_kind, parent_kind).map(|policy| {
                            (
                                policy.projection.parent_preference_rank,
                                entity_type_sort_rank(&candidate.entity_type),
                            )
                        })
                    })
                    .unwrap_or((usize::MAX, entity_type_sort_rank(&candidate.entity_type)));
                Some((pair_projection, candidate.id.clone()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    parent_ids.sort_by(|left, right| {
        let left_node = nodes_by_id
            .get(&left.1)
            .expect("left hierarchy parent exists");
        let right_node = nodes_by_id
            .get(&right.1)
            .expect("right hierarchy parent exists");
        left.0.cmp(&right.0).then_with(|| {
            preferred_parent_sort_key(left_node).cmp(&preferred_parent_sort_key(right_node))
        })
    });
    parent_ids.dedup_by(|left, right| left.1 == right.1);
    parent_ids.into_iter().map(|(_, id)| id).collect()
}

pub(super) fn build_preferred_hierarchy_parent_map(
    nodes: &[SollDocNode],
    outgoing: &HashMap<String, Vec<SollDocEdge>>,
    nodes_by_id: &HashMap<String, SollDocNode>,
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for node in nodes {
        let parent_ids = hierarchy_candidate_parent_ids(&node.id, outgoing, nodes_by_id);
        if let Some(parent_id) = parent_ids.first() {
            map.insert(node.id.clone(), parent_id.clone());
        }
    }
    map
}

pub(super) fn build_hierarchy_children_map(
    preferred_parent_map: &HashMap<String, String>,
    nodes_by_id: &HashMap<String, SollDocNode>,
) -> HashMap<String, Vec<String>> {
    let mut children = HashMap::<String, Vec<String>>::new();
    for (child_id, parent_id) in preferred_parent_map {
        children
            .entry(parent_id.clone())
            .or_default()
            .push(child_id.clone());
    }
    for child_ids in children.values_mut() {
        child_ids.sort_by(|left, right| {
            let left_node = nodes_by_id.get(left).expect("child node exists");
            let right_node = nodes_by_id.get(right).expect("child node exists");
            (
                entity_type_sort_rank(&left_node.entity_type),
                left_node.id.as_str(),
                left_node.title.as_str(),
            )
                .cmp(&(
                    entity_type_sort_rank(&right_node.entity_type),
                    right_node.id.as_str(),
                    right_node.title.as_str(),
                ))
        });
        child_ids.dedup();
    }
    children
}

pub(super) fn hierarchy_root_ids_for_project(
    nodes: &[SollDocNode],
    preferred_parent_map: &HashMap<String, String>,
) -> Vec<String> {
    let mut canonical_roots = nodes
        .iter()
        .filter(|node| {
            !preferred_parent_map.contains_key(&node.id)
                && entity_type_to_kind(&node.entity_type)
                    .and_then(kind_projection_policy)
                    .is_some_and(|policy| policy.root_eligible)
        })
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let mut fallback_roots = nodes
        .iter()
        .filter(|node| {
            !preferred_parent_map.contains_key(&node.id)
                && !entity_type_to_kind(&node.entity_type)
                    .and_then(kind_projection_policy)
                    .is_some_and(|policy| policy.root_eligible)
        })
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    canonical_roots.sort();
    fallback_roots.sort();
    if canonical_roots.is_empty() {
        fallback_roots
    } else {
        canonical_roots
    }
}

pub(super) fn hierarchy_unattached_ids_for_project(
    nodes: &[SollDocNode],
    preferred_parent_map: &HashMap<String, String>,
) -> Vec<String> {
    let mut unattached = nodes
        .iter()
        .filter(|node| {
            !preferred_parent_map.contains_key(&node.id)
                && !entity_type_to_kind(&node.entity_type)
                    .and_then(kind_projection_policy)
                    .is_some_and(|policy| policy.root_eligible)
        })
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    unattached.sort();
    unattached
}

pub(super) fn ancestor_chain_ids(
    current_node_id: &str,
    preferred_parent_map: &HashMap<String, String>,
) -> HashSet<String> {
    let mut expanded = HashSet::new();
    let mut cursor = Some(current_node_id.to_string());
    while let Some(node_id) = cursor {
        expanded.insert(node_id.clone());
        cursor = preferred_parent_map.get(&node_id).cloned();
    }
    expanded
}

pub(super) fn subtree_anchor_type(entity_type: &str) -> bool {
    if entity_type_to_kind(entity_type)
        .and_then(kind_projection_policy)
        .is_some_and(|policy| policy.root_eligible)
    {
        return true;
    }
    let parent_kind = entity_type_to_kind("Vision");
    let candidate_kind = entity_type_to_kind(entity_type);
    match (candidate_kind, parent_kind) {
        (Some(source_kind), Some(target_kind)) => {
            relation_policy_for_pair(source_kind, target_kind)
                .is_some_and(|policy| matches!(policy.projection.role, ProjectionRole::Primary))
        }
        _ => false,
    }
}

fn relation_diagnostic_row(
    edge: &SollDocEdge,
    nodes_by_id: &HashMap<String, SollDocNode>,
) -> Value {
    let relation_type = edge.relation_type.as_str();
    let source_kind = nodes_by_id
        .get(&edge.source_id)
        .and_then(|node| entity_type_to_kind(&node.entity_type));
    let target_kind = nodes_by_id
        .get(&edge.target_id)
        .and_then(|node| entity_type_to_kind(&node.entity_type));
    let policy = source_kind
        .zip(target_kind)
        .and_then(|(source_kind, target_kind)| relation_policy_for_pair(source_kind, target_kind));
    let projection_role = policy
        .map(|policy| policy.projection.role.as_str())
        .unwrap_or("derived");
    let is_primary = projection_role == "primary";
    let score_bearing = matches!(
        relation_type,
        "EPITOMIZES" | "BELONGS_TO" | "SOLVES" | "VERIFIES" | "IMPACTS"
    );
    json!({
        "relation_type": relation_type,
        "source_id": edge.source_id,
        "target_id": edge.target_id,
        "boundary": if policy.is_some() { "canonical" } else { "derived" },
        "projection_role": projection_role,
        "score_class": if score_bearing { "score_bearing" } else { "non_score_bearing" },
        "primary_class": if is_primary { "primary" } else { "supporting_or_lateral" }
    })
}

pub(super) fn relation_diagnostic_table_html(
    edges: &[SollDocEdge],
    nodes_by_id: &HashMap<String, SollDocNode>,
) -> String {
    if edges.is_empty() {
        return "<p class=\"muted\">No relation diagnostics in this scope.</p>".to_string();
    }
    let mut items = edges
        .iter()
        .map(|edge| relation_diagnostic_row(edge, nodes_by_id))
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        (
            left["relation_type"].as_str().unwrap_or_default(),
            left["source_id"].as_str().unwrap_or_default(),
            left["target_id"].as_str().unwrap_or_default(),
        )
            .cmp(&(
                right["relation_type"].as_str().unwrap_or_default(),
                right["source_id"].as_str().unwrap_or_default(),
                right["target_id"].as_str().unwrap_or_default(),
            ))
    });

    let mut html = String::from(
        "<section class=\"card\"><h3>Operator Relation Diagnostics</h3><ul class=\"relation-list\">",
    );
    for item in items {
        html.push_str(&format!(
            "<li><code>{}</code> <span class=\"rel\">{}</span> <code>{}</code>\
             <div class=\"muted\">boundary: {} · projection: {} · scoring: {}</div></li>",
            html_escape(item["source_id"].as_str().unwrap_or_default()),
            html_escape(item["relation_type"].as_str().unwrap_or_default()),
            html_escape(item["target_id"].as_str().unwrap_or_default()),
            html_escape(item["boundary"].as_str().unwrap_or_default()),
            html_escape(item["projection_role"].as_str().unwrap_or_default()),
            html_escape(item["score_class"].as_str().unwrap_or_default())
        ));
    }
    html.push_str("</ul></section>");
    html
}

fn find_path_to_root_via_outgoing(
    start_id: &str,
    root_id: &str,
    outgoing: &HashMap<String, Vec<SollDocEdge>>,
) -> Option<Vec<SollDocEdge>> {
    if start_id == root_id {
        return Some(Vec::new());
    }
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back((start_id.to_string(), Vec::<SollDocEdge>::new()));
    while let Some((current, path)) = queue.pop_front() {
        if !visited.insert(current.clone()) {
            continue;
        }
        if let Some(edges) = outgoing.get(&current) {
            for edge in edges {
                let mut next_path = path.clone();
                next_path.push(edge.clone());
                if edge.target_id == root_id {
                    return Some(next_path);
                }
                queue.push_back((edge.target_id.clone(), next_path));
            }
        }
    }
    None
}

pub(super) fn subtree_membership_reason_html(
    root: &SollDocNode,
    member_id: &str,
    outgoing: &HashMap<String, Vec<SollDocEdge>>,
    nodes_by_id: &HashMap<String, SollDocNode>,
) -> String {
    if member_id == root.id {
        return format!(
            "Included because this node is the subtree root <code>{}</code>.",
            html_escape(&root.id)
        );
    }
    if let Some(edges) = outgoing.get(&root.id) {
        if let Some(edge) = edges.iter().find(|edge| edge.target_id == member_id) {
            return format!(
                "Included by direct root context edge <code>{}</code> <span class=\"rel\">{}</span> <code>{}</code>.",
                html_escape(&edge.source_id),
                html_escape(&edge.relation_type),
                html_escape(&edge.target_id)
            );
        }
    }
    if let Some(path) = find_path_to_root_via_outgoing(member_id, &root.id, outgoing) {
        if let Some(first_edge) = path.first() {
            let target_label = nodes_by_id
                .get(&first_edge.target_id)
                .map(|node| {
                    format!(
                        "{} · {}",
                        entity_type_short_label(&node.entity_type),
                        node.title
                    )
                })
                .unwrap_or_else(|| first_edge.target_id.clone());
            return format!(
                "Included by reverse reachability toward root <code>{}</code>; canonical chain starts with <code>{}</code> <span class=\"rel\">{}</span> <code>{}</code> ({}) .",
                html_escape(&root.id),
                html_escape(&first_edge.source_id),
                html_escape(&first_edge.relation_type),
                html_escape(&first_edge.target_id),
                html_escape(&target_label)
            );
        }
    }
    "Included by subtree derivation logic, but no explicit canonical path explanation is currently available.".to_string()
}

fn render_tree_node_html(
    node_id: &str,
    children_map: &HashMap<String, Vec<String>>,
    nodes_by_id: &HashMap<String, SollDocNode>,
    current_node_id: Option<&str>,
    node_href_prefix: &str,
    expanded_nodes: &HashSet<String>,
) -> String {
    let Some(node) = nodes_by_id.get(node_id) else {
        return String::new();
    };
    let child_ids = children_map.get(node_id).cloned().unwrap_or_default();
    let is_current = current_node_id.is_some_and(|candidate| candidate == node_id);
    let current_class = if is_current { " current" } else { "" };
    let label_html = format!(
        "<a class=\"tree-link{}\" href=\"{}{}\"><span class=\"tree-tag\">{}</span><span>{}</span></a>",
        current_class,
        node_href_prefix,
        html_escape(&node_file_name(&node.id)),
        html_escape(entity_type_short_label(&node.entity_type)),
        html_escape(&node.title)
    );
    if child_ids.is_empty() {
        return format!("<li class=\"tree-item leaf\">{}</li>", label_html);
    }

    let child_html = child_ids
        .iter()
        .map(|child_id| {
            render_tree_node_html(
                child_id,
                children_map,
                nodes_by_id,
                current_node_id,
                node_href_prefix,
                expanded_nodes,
            )
        })
        .collect::<String>();
    let open_attr = if expanded_nodes.contains(node_id) {
        " open"
    } else {
        ""
    };
    format!(
        "<li class=\"tree-item branch\"><details{}><summary>{}</summary><ul class=\"tree-children\">{}</ul></details></li>",
        open_attr, label_html, child_html
    )
}

pub(super) fn render_project_tree_html(
    project_code: &str,
    root_ids: &[String],
    children_map: &HashMap<String, Vec<String>>,
    nodes_by_id: &HashMap<String, SollDocNode>,
    current_node_id: Option<&str>,
    node_href_prefix: &str,
    project_root_href: &str,
    default_open: bool,
    expanded_nodes: &HashSet<String>,
) -> String {
    let root_children_html = root_ids
        .iter()
        .map(|root_id| {
            render_tree_node_html(
                root_id,
                children_map,
                nodes_by_id,
                current_node_id,
                node_href_prefix,
                expanded_nodes,
            )
        })
        .collect::<String>();
    let open_attr = if default_open { " open" } else { "" };
    format!(
        "<nav class=\"tree-shell\" aria-label=\"Project hierarchy\"><ul class=\"tree-root\">\
           <li class=\"tree-item branch root\"><details{}>\
             <summary><a class=\"tree-link{}\" href=\"{}\"><span class=\"tree-tag\">PRJ</span><span>{}</span></a></summary>\
             <ul class=\"tree-children\">{}</ul>\
           </details></li>\
         </ul></nav>",
        open_attr,
        if current_node_id.is_none() { " current" } else { "" },
        html_escape(project_root_href),
        html_escape(project_code),
        root_children_html
    )
}

pub(super) fn relation_line_html(
    edges: &[SollDocEdge],
    nodes_by_id: &HashMap<String, SollDocNode>,
) -> String {
    if edges.is_empty() {
        return "<p class=\"muted\">No relations in this scope.</p>".to_string();
    }

    let mut items = edges.to_vec();
    items.sort_by(|left, right| {
        (&left.relation_type, &left.source_id, &left.target_id).cmp(&(
            &right.relation_type,
            &right.source_id,
            &right.target_id,
        ))
    });

    let mut html = String::from("<ul class=\"relation-list\">");
    for edge in items {
        let source_label = nodes_by_id
            .get(&edge.source_id)
            .map(|node| {
                format!(
                    "{} · {}",
                    entity_type_short_label(&node.entity_type),
                    node.title
                )
            })
            .unwrap_or_else(|| edge.source_id.clone());
        let target_label = nodes_by_id
            .get(&edge.target_id)
            .map(|node| {
                format!(
                    "{} · {}",
                    entity_type_short_label(&node.entity_type),
                    node.title
                )
            })
            .unwrap_or_else(|| edge.target_id.clone());
        html.push_str(&format!(
            "<li><code>{}</code> <span class=\"rel\">{}</span> <code>{}</code><div class=\"muted\">{} -> {}</div></li>",
            html_escape(&edge.source_id),
            html_escape(&edge.relation_type),
            html_escape(&edge.target_id),
            html_escape(&source_label),
            html_escape(&target_label)
        ));
    }
    html.push_str("</ul>");
    html
}

pub(super) fn linked_node_list_html(
    title: &str,
    node_ids: &[String],
    nodes_by_id: &HashMap<String, SollDocNode>,
    page_prefix: &str,
) -> String {
    if node_ids.is_empty() {
        return format!(
            "<section class=\"card\"><h3>{}</h3><p class=\"muted\">None.</p></section>",
            html_escape(title)
        );
    }

    let mut ids = node_ids.to_vec();
    ids.sort();
    ids.dedup();

    let mut html = format!(
        "<section class=\"card\"><h3>{}</h3><ul class=\"node-list\">",
        html_escape(title)
    );
    for node_id in ids {
        let Some(node) = nodes_by_id.get(&node_id) else {
            continue;
        };
        html.push_str(&format!(
            "<li><a href=\"{}{}\">{} · {}</a><span class=\"muted\">{}</span></li>",
            page_prefix,
            html_escape(&node_file_name(&node.id)),
            html_escape(entity_type_short_label(&node.entity_type)),
            html_escape(&node.title),
            html_escape(&node.id)
        ));
    }
    html.push_str("</ul></section>");
    html
}

pub(super) fn linked_page_list_html(title: &str, items: &[(String, String, String)]) -> String {
    if items.is_empty() {
        return format!(
            "<section class=\"card\"><h3>{}</h3><p class=\"muted\">None.</p></section>",
            html_escape(title)
        );
    }

    let mut ordered = items.to_vec();
    ordered.sort_by(|left, right| (&left.1, &left.0).cmp(&(&right.1, &right.0)));
    ordered.dedup_by(|left, right| left.0 == right.0);

    let mut html = format!(
        "<section class=\"card\"><h3>{}</h3><ul class=\"node-list\">",
        html_escape(title)
    );
    for (href, label, meta) in ordered {
        html.push_str(&format!(
            "<li><a href=\"{}\">{}</a><span class=\"muted\">{}</span></li>",
            html_escape(&href),
            html_escape(&label),
            html_escape(&meta)
        ));
    }
    html.push_str("</ul></section>");
    html
}
