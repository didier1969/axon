use super::*;

impl McpServer {
    pub(super) fn load_soll_doc_nodes(
        &self,
        project_code: &str,
    ) -> Result<Vec<SollDocNode>, String> {
        let raw = self
            .graph_store
            .query_json(&format!(
                "SELECT id, type, COALESCE(title, ''), COALESCE(description, ''), COALESCE(status, ''), COALESCE(metadata, '{{}}') \
                 FROM soll.Node{} ORDER BY type, id",
                project_scope_clause_for_table("id", Some(project_code))
            ))
            .map_err(|e| e.to_string())?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|row| row.len() >= 6)
            .map(|row| SollDocNode {
                id: row[0].clone(),
                entity_type: row[1].clone(),
                title: row[2].clone(),
                description: row[3].clone(),
                status: row[4].clone(),
                metadata: row[5].clone(),
            })
            .collect())
    }

    pub(super) fn load_soll_doc_edges(
        &self,
        project_code: &str,
    ) -> Result<Vec<SollDocEdge>, String> {
        let raw = self
            .graph_store
            .query_json(&format!(
                "SELECT source_id, target_id, relation_type FROM soll.Edge{}",
                project_scope_clause_for_relation(Some(project_code))
            ))
            .map_err(|e| e.to_string())?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|row| row.len() >= 3)
            .map(|row| SollDocEdge {
                source_id: row[0].clone(),
                target_id: row[1].clone(),
                relation_type: row[2].clone(),
            })
            .collect())
    }

    pub(super) fn generate_soll_doc_pages(
        &self,
        project_code: &str,
        nodes: &[SollDocNode],
        edges: &[SollDocEdge],
    ) -> Vec<SollDocPageSpec> {
        let nodes_by_id = nodes
            .iter()
            .map(|node| (node.id.clone(), node.clone()))
            .collect::<HashMap<_, _>>();
        let mut incoming = HashMap::<String, Vec<SollDocEdge>>::new();
        let mut outgoing = HashMap::<String, Vec<SollDocEdge>>::new();
        for edge in edges {
            incoming
                .entry(edge.target_id.clone())
                .or_default()
                .push(edge.clone());
            outgoing
                .entry(edge.source_id.clone())
                .or_default()
                .push(edge.clone());
        }

        let preferred_parent_map =
            build_preferred_hierarchy_parent_map(nodes, &outgoing, &nodes_by_id);
        let hierarchy_children_map =
            build_hierarchy_children_map(&preferred_parent_map, &nodes_by_id);
        let hierarchy_root_ids = hierarchy_root_ids_for_project(nodes, &preferred_parent_map);
        let unattached_ids = hierarchy_unattached_ids_for_project(nodes, &preferred_parent_map);

        let mut pages = Vec::new();
        let project_summary = {
            let counts = nodes
                .iter()
                .fold(HashMap::<String, usize>::new(), |mut acc, node| {
                    *acc.entry(node.entity_type.clone()).or_insert(0) += 1;
                    acc
                });
            let mut items = counts.into_iter().collect::<Vec<_>>();
            items.sort_by(|left, right| left.0.cmp(&right.0));
            items
                .into_iter()
                .map(|(kind, count)| {
                    format!(
                        "<div class=\"cell\"><strong>{}</strong><div>{}</div></div>",
                        html_escape(&kind),
                        count
                    )
                })
                .collect::<String>()
        };

        let project_tree_html = render_project_tree_html(
            project_code,
            &hierarchy_root_ids,
            &hierarchy_children_map,
            &nodes_by_id,
            None,
            "nodes/",
            "index.html",
            true,
            &HashSet::new(),
        );
        let project_focus_nodes = hierarchy_root_ids
            .iter()
            .filter_map(|root_id| nodes_by_id.get(root_id).cloned())
            .collect::<Vec<_>>();
        let mut project_graph_nodes = vec![SollDocNode {
            id: format!("PRJ-{}", project_code),
            entity_type: "Project".to_string(),
            title: project_code.to_string(),
            description: format!("Derived project root for {}", project_code),
            status: "derived".to_string(),
            metadata: "{}".to_string(),
        }];
        project_graph_nodes.extend(project_focus_nodes.clone());
        let project_graph_edges = project_focus_nodes
            .iter()
            .map(|node| SollDocEdge {
                source_id: format!("PRJ-{}", project_code),
                target_id: node.id.clone(),
                relation_type: "CONTAINS".to_string(),
            })
            .collect::<Vec<_>>();
        let project_graph_links = project_focus_nodes
            .iter()
            .map(|node| {
                (
                    node.id.clone(),
                    format!("nodes/{}", node_file_name(&node.id)),
                )
            })
            .chain(std::iter::once((
                format!("PRJ-{}", project_code),
                "index.html".to_string(),
            )))
            .collect::<HashMap<_, _>>();
        let project_right_html = format!(
            "{}{}{}{}{}",
            linked_node_list_html("Vision Children", &hierarchy_root_ids, &nodes_by_id, "nodes/"),
            linked_node_list_html(
                "Unattached Node Pages",
                &unattached_ids,
                &nodes_by_id,
                "nodes/"
            ),
            linked_node_list_html(
                "All Node Pages",
                &nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>(),
                &nodes_by_id,
                "nodes/"
            ),
            linked_page_list_html(
                "Compatibility Subtree Pages",
                &hierarchy_root_ids
                    .iter()
                    .filter_map(|node_id| nodes_by_id.get(node_id))
                    .map(|node| (
                        format!("subtrees/{}", subtree_file_name(&node.id)),
                        format!(
                            "{} subtree · {}",
                            entity_type_short_label(&node.entity_type),
                            node.title
                        ),
                        node.id.clone()
                    ))
                    .collect::<Vec<_>>()
            ),
            "<section class=\"card\"><h3>Reading Model</h3><p class=\"muted\">Project root on the left, attached visions on the right. Use the tree to descend, or click a focus child to open its own page.</p></section>"
        );
        pages.push(SollDocPageSpec {
            relative_path: "index.html".to_string(),
            title: format!("{} Project Root", project_code),
            html: render_site_page(
                &format!("{} Project Root", project_code),
                "SOLL Derived Project",
                "Project-level hierarchy page derived from live SOLL. This is a human-readable navigation surface, not canonical truth.",
                &format!(
                    "<a href=\"../index.html\">GLO</a><span>/</span><span>{}</span>",
                    html_escape(project_code)
                ),
                "Project Tree",
                &project_tree_html,
                "Hierarchy Focus",
                &render_mermaid_graph(
                    &project_graph_nodes,
                    &project_graph_edges,
                    &project_graph_links,
                    None,
                ),
                "Details",
                &project_right_html,
                &format!(
                    "{}<div class=\"cell\"><strong>Focus</strong><div>{}</div></div><div class=\"cell\"><strong>Boundary</strong><div>derived / non-canonical</div></div>",
                    project_summary,
                    html_escape(project_code)
                ),
            ),
            node_ids: project_focus_nodes.iter().map(|node| node.id.clone()).collect(),
            edge_keys: project_graph_edges.iter().map(edge_key).collect(),
        });

        let mut subtree_roots = nodes
            .iter()
            .filter(|node| subtree_anchor_type(&node.entity_type))
            .cloned()
            .collect::<Vec<_>>();
        subtree_roots.sort_by(|left, right| left.id.cmp(&right.id));
        let mut subtree_membership = HashMap::<String, Vec<String>>::new();
        for root in subtree_roots {
            let mut subtree_ids = HashSet::new();
            let mut queue = vec![root.id.clone()];
            while let Some(current) = queue.pop() {
                if !subtree_ids.insert(current.clone()) {
                    continue;
                }
                if let Some(parent_edges) = incoming.get(&current) {
                    queue.extend(parent_edges.iter().map(|edge| edge.source_id.clone()));
                }
            }
            if let Some(root_outgoing) = outgoing.get(&root.id) {
                subtree_ids.extend(root_outgoing.iter().map(|edge| edge.target_id.clone()));
            }
            for node_id in &subtree_ids {
                subtree_membership
                    .entry(node_id.clone())
                    .or_default()
                    .push(root.id.clone());
            }

            let mut subtree_nodes = subtree_ids
                .iter()
                .filter_map(|id| nodes_by_id.get(id).cloned())
                .collect::<Vec<_>>();
            subtree_nodes.sort_by(|left, right| left.id.cmp(&right.id));
            let subtree_edges = edges
                .iter()
                .filter(|edge| {
                    subtree_ids.contains(&edge.source_id) && subtree_ids.contains(&edge.target_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            let subtree_links = subtree_nodes
                .iter()
                .map(|node| {
                    (
                        node.id.clone(),
                        format!("../nodes/{}", node_file_name(&node.id)),
                    )
                })
                .collect::<HashMap<_, _>>();
            let inbound_ids = incoming
                .get(&root.id)
                .map(|items| {
                    items
                        .iter()
                        .map(|edge| edge.source_id.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let outbound_ids = outgoing
                .get(&root.id)
                .map(|items| {
                    items
                        .iter()
                        .map(|edge| edge.target_id.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let hierarchy_root_set = hierarchy_root_ids.iter().cloned().collect::<HashSet<_>>();
            let related_subtree_items = inbound_ids
                .iter()
                .chain(outbound_ids.iter())
                .filter(|candidate| hierarchy_root_set.contains(*candidate))
                .filter_map(|candidate| nodes_by_id.get(candidate))
                .map(|candidate| {
                    (
                        subtree_file_name(&candidate.id),
                        format!(
                            "{} · {}",
                            entity_type_short_label(&candidate.entity_type),
                            candidate.title
                        ),
                        candidate.id.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let subtree_node_ids = subtree_nodes
                .iter()
                .map(|node| node.id.clone())
                .collect::<Vec<_>>();
            let subtree_inclusion_reason_items = subtree_nodes
                .iter()
                .map(|member| {
                    format!(
                        "<li><a href=\"../nodes/{}\">{}</a><div class=\"muted\">{}</div></li>",
                        html_escape(&node_file_name(&member.id)),
                        html_escape(&format!(
                            "{} · {}",
                            entity_type_short_label(&member.entity_type),
                            member.title
                        )),
                        subtree_membership_reason_html(&root, &member.id, &outgoing, &nodes_by_id)
                    )
                })
                .collect::<String>();
            let left_tree_html = render_project_tree_html(
                project_code,
                &hierarchy_root_ids,
                &hierarchy_children_map,
                &nodes_by_id,
                Some(&root.id),
                "../nodes/",
                "../index.html",
                true,
                &ancestor_chain_ids(&root.id, &preferred_parent_map),
            );
            let right_html = format!(
                "{}{}{}{}<section class=\"card\"><h3>Subtree Inclusion Reasons</h3><ul class=\"node-list\">{}</ul></section><section class=\"card\"><h3>Relations</h3>{}</section>{}",
                linked_page_list_html("Related Subtrees", &related_subtree_items),
                linked_node_list_html(
                    "Feeds Into This Root",
                    &inbound_ids,
                    &nodes_by_id,
                    "../nodes/"
                ),
                linked_node_list_html(
                    "Root Outgoing Context",
                    &outbound_ids,
                    &nodes_by_id,
                    "../nodes/"
                ),
                linked_node_list_html(
                    "All Nodes In This Subtree",
                    &subtree_node_ids,
                    &nodes_by_id,
                    "../nodes/"
                ),
                subtree_inclusion_reason_items,
                relation_line_html(&subtree_edges, &nodes_by_id),
                relation_diagnostic_table_html(&subtree_edges, &nodes_by_id)
            );
            let summary_html = format!(
                "<div class=\"cell\"><strong>Root</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Nodes</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Edges</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Boundary</strong><div>derived / non-canonical</div></div>\
                 <div class=\"cell\"><strong>Diagnostics</strong><div>subtree inclusion reasons + relation tagging</div></div>",
                html_escape(&root.id),
                subtree_nodes.len(),
                subtree_edges.len()
            );
            let subtree_graph =
                render_mermaid_graph(&subtree_nodes, &subtree_edges, &subtree_links, None);
            pages.push(SollDocPageSpec {
                relative_path: format!("subtrees/{}", subtree_file_name(&root.id)),
                title: format!("{} · {} subtree", root.id, root.title),
                html: render_site_page(
                    &format!("{} · {}", root.id, root.title),
                    "SOLL Derived Subtree",
                    "Compatibility projection around a subtree root. This view is generated for navigation and diagnostics only.",
                    &format!(
                        "<a href=\"../../index.html\">GLO</a><span>/</span><a href=\"../index.html\">{}</a><span>/</span><span>{}</span>",
                        html_escape(project_code),
                        html_escape(&root.id)
                    ),
                    "Project Tree",
                    &left_tree_html,
                    "Subtree Focus",
                    &subtree_graph,
                    "Details",
                    &right_html,
                    &summary_html,
                ),
                node_ids: subtree_nodes.iter().map(|node| node.id.clone()).collect(),
                edge_keys: subtree_edges.iter().map(edge_key).collect(),
            });
        }

        for node in nodes {
            let parent_ids = hierarchy_candidate_parent_ids(&node.id, &outgoing, &nodes_by_id);
            let child_ids = hierarchy_children_map
                .get(&node.id)
                .cloned()
                .unwrap_or_default();
            let incoming_ids = incoming
                .get(&node.id)
                .map(|items| {
                    items
                        .iter()
                        .map(|edge| edge.source_id.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let outgoing_ids = outgoing
                .get(&node.id)
                .map(|items| {
                    items
                        .iter()
                        .map(|edge| edge.target_id.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let containing_roots = subtree_membership
                .get(&node.id)
                .cloned()
                .unwrap_or_default();

            let mut local_ids = HashSet::new();
            local_ids.insert(node.id.clone());
            local_ids.extend(parent_ids.iter().cloned());
            local_ids.extend(child_ids.iter().cloned());
            local_ids.extend(incoming_ids.iter().cloned());
            local_ids.extend(outgoing_ids.iter().cloned());
            local_ids.extend(containing_roots.iter().cloned());

            let mut local_nodes = local_ids
                .iter()
                .filter_map(|id| nodes_by_id.get(id).cloned())
                .collect::<Vec<_>>();
            local_nodes.sort_by(|left, right| left.id.cmp(&right.id));
            let mut local_edges = edges
                .iter()
                .filter(|edge| {
                    local_ids.contains(&edge.source_id) && local_ids.contains(&edge.target_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            local_edges.sort_by(|left, right| edge_key(left).cmp(&edge_key(right)));
            local_edges.dedup_by(|left, right| edge_key(left) == edge_key(right));

            let local_links = local_nodes
                .iter()
                .map(|candidate| (candidate.id.clone(), node_file_name(&candidate.id)))
                .collect::<HashMap<_, _>>();
            let containing_subtree_items = containing_roots
                .iter()
                .filter_map(|root_id| nodes_by_id.get(root_id))
                .map(|root_node| {
                    (
                        format!("../subtrees/{}", subtree_file_name(&root_node.id)),
                        format!(
                            "{} · {}",
                            entity_type_short_label(&root_node.entity_type),
                            root_node.title
                        ),
                        subtree_membership_reason_html(
                            root_node,
                            &node.id,
                            &outgoing,
                            &nodes_by_id,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            let parent_page_items = parent_ids
                .iter()
                .filter_map(|parent_id| nodes_by_id.get(parent_id))
                .map(|candidate| {
                    (
                        node_file_name(&candidate.id),
                        format!(
                            "{} · {}",
                            entity_type_short_label(&candidate.entity_type),
                            candidate.title
                        ),
                        candidate.id.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let child_page_items = child_ids
                .iter()
                .filter_map(|child_id| nodes_by_id.get(child_id))
                .map(|candidate| {
                    (
                        node_file_name(&candidate.id),
                        format!(
                            "{} · {}",
                            entity_type_short_label(&candidate.entity_type),
                            candidate.title
                        ),
                        candidate.id.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let summary_html = format!(
                "<div class=\"cell\"><strong>Kind</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Status</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Relations</strong><div>{}</div></div>\
                 <div class=\"cell\"><strong>Boundary</strong><div>derived / non-canonical</div></div>",
                html_escape(&node.entity_type),
                html_escape(if node.status.is_empty() {
                    "unknown"
                } else {
                    &node.status
                }),
                local_edges.len()
            );
            let left_tree_html = render_project_tree_html(
                project_code,
                &hierarchy_root_ids,
                &hierarchy_children_map,
                &nodes_by_id,
                Some(&node.id),
                "",
                "../index.html",
                true,
                &ancestor_chain_ids(&node.id, &preferred_parent_map),
            );
            let right_html = format!(
                "<section class=\"card\"><h3>Description</h3><p>{}</p></section>\
                 <section class=\"card\"><h3>Metadata</h3><pre>{}</pre></section>\
                 {}{}{}{}{}{}{}",
                html_escape(if node.description.is_empty() {
                    "No description."
                } else {
                    &node.description
                }),
                html_escape(&node.metadata),
                linked_page_list_html("Containing Subtrees", &containing_subtree_items),
                linked_page_list_html("Primary Parent Node Pages", &parent_page_items),
                linked_page_list_html("Primary Child Node Pages", &child_page_items),
                linked_node_list_html("Primary Hierarchy Parents", &parent_ids, &nodes_by_id, ""),
                linked_node_list_html("Primary Hierarchy Children", &child_ids, &nodes_by_id, ""),
                linked_node_list_html("Incoming Neighbors", &incoming_ids, &nodes_by_id, ""),
                linked_node_list_html("Outgoing Neighbors", &outgoing_ids, &nodes_by_id, ""),
            );
            let relation_html = format!(
                "{}{}",
                relation_line_html(&local_edges, &nodes_by_id),
                relation_diagnostic_table_html(&local_edges, &nodes_by_id)
            );
            // REQ-AXO-312 — partition the level ±1 neighbourhood so the local
            // graph renders macro (parents / upstream / containing roots) on
            // the left and micro (children / downstream) on the right.
            let macro_ids = parent_ids
                .iter()
                .chain(incoming_ids.iter())
                .chain(containing_roots.iter())
                .filter(|id| **id != node.id)
                .cloned()
                .collect::<HashSet<_>>();
            let micro_ids = child_ids
                .iter()
                .chain(outgoing_ids.iter())
                .filter(|id| **id != node.id && !macro_ids.contains(*id))
                .cloned()
                .collect::<HashSet<_>>();
            let node_focus = MermaidFocus {
                focus_id: node.id.clone(),
                macro_ids,
                micro_ids,
            };
            let node_graph =
                render_mermaid_graph(&local_nodes, &local_edges, &local_links, Some(&node_focus));
            pages.push(SollDocPageSpec {
                relative_path: format!("nodes/{}", node_file_name(&node.id)),
                title: format!("{} · {}", node.id, node.title),
                html: render_site_page(
                    &format!("{} · {}", node.id, node.title),
                    "SOLL Derived Node",
                    "Generated node page combining hierarchy, local context, and relation diagnostics.",
                    &format!(
                        "<a href=\"../../index.html\">GLO</a><span>/</span><a href=\"../index.html\">{}</a><span>/</span><span>{}</span>",
                        html_escape(project_code),
                        html_escape(&node.id)
                    ),
                    "Project Tree",
                    &left_tree_html,
                    "Local Graph",
                    &node_graph,
                    "Details",
                    &format!(
                        "{}<section class=\"card\"><h3>Relations</h3>{}</section>",
                        right_html, relation_html
                    ),
                    &summary_html,
                ),
                node_ids: local_nodes.iter().map(|candidate| candidate.id.clone()).collect(),
                edge_keys: local_edges.iter().map(edge_key).collect(),
            });
        }

        pages.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        pages
    }
}
