use super::*;

impl McpServer {
    fn delete_obsolete_derived_doc_paths(
        &self,
        manifest_path: &Path,
        output_root: &Path,
        current_relative_paths: &HashSet<String>,
    ) -> Result<Vec<String>, String> {
        let existing_manifest = match std::fs::read_to_string(manifest_path) {
            Ok(contents) => contents,
            Err(_) => return Ok(Vec::new()),
        };
        let manifest: Value =
            serde_json::from_str(&existing_manifest).unwrap_or_else(|_| json!({}));
        let mut deleted = Vec::new();
        if let Some(pages) = manifest.get("pages").and_then(|value| value.as_array()) {
            for relative_path in pages
                .iter()
                .filter_map(|page| page.get("path").and_then(|value| value.as_str()))
            {
                if current_relative_paths.contains(relative_path) {
                    continue;
                }
                let stale_path = output_root.join(relative_path);
                if stale_path.is_file() {
                    std::fs::remove_file(&stale_path).map_err(|error| error.to_string())?;
                    deleted.push(stale_path.to_string_lossy().to_string());
                }
            }
        }
        Ok(deleted)
    }

    fn should_use_incremental_project_docs(&self, manifest_path: &Path) -> bool {
        let Ok(existing_manifest) = std::fs::read_to_string(manifest_path) else {
            return false;
        };
        let Ok(manifest) = serde_json::from_str::<Value>(&existing_manifest) else {
            return false;
        };
        manifest
            .get("generator_version")
            .and_then(|value| value.as_str())
            .map(|value| value == SOLL_PROJECT_DOCS_GENERATOR_VERSION)
            .unwrap_or(false)
            && manifest
                .get("pages")
                .and_then(|value| value.as_array())
                .is_some()
    }

    fn load_soll_derived_project_entries(&self, site_root: &Path) -> Vec<SollDerivedProjectEntry> {
        let _ = self.sync_project_code_registry_from_meta();
        let registry_raw = self
            .graph_store
            .query_json(
                "SELECT project_code, COALESCE(project_name,''), COALESCE(project_path,'') \
                 FROM soll.ProjectCodeRegistry ORDER BY project_code ASC, project_name ASC",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let registry_rows: Vec<Vec<String>> =
            serde_json::from_str(&registry_raw).unwrap_or_default();

        let counts_raw = self
            .graph_store
            .query_json(
                "SELECT project_code, CAST(COUNT(*) AS TEXT) FROM soll.Node GROUP BY project_code ORDER BY project_code ASC",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let count_rows: Vec<Vec<String>> = serde_json::from_str(&counts_raw).unwrap_or_default();
        let node_counts = count_rows
            .into_iter()
            .filter(|row| row.len() >= 2)
            .map(|row| (row[0].clone(), row[1].parse::<usize>().unwrap_or_default()))
            .collect::<HashMap<_, _>>();

        let mut entries = registry_rows
            .into_iter()
            .filter(|row| row.len() >= 3)
            .map(|row| {
                let project_code = row[0].clone();
                let has_docs = site_root.join(&project_code).join("index.html").is_file();
                SollDerivedProjectEntry {
                    node_count: *node_counts.get(&project_code).unwrap_or(&0),
                    has_docs,
                    project_code,
                    project_name: row[1].clone(),
                    project_path: row[2].clone(),
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            (&left.project_code, &left.project_name)
                .cmp(&(&right.project_code, &right.project_name))
        });
        entries
    }

    fn render_soll_root_page(&self, entries: &[SollDerivedProjectEntry]) -> String {
        let mut graph_nodes = vec![SollDocNode {
            id: "GLO".to_string(),
            entity_type: "Portfolio".to_string(),
            title: "Global portfolio".to_string(),
            description: "Derived reading root for all known projects".to_string(),
            status: "derived".to_string(),
            metadata: "{}".to_string(),
        }];
        let mut graph_edges = Vec::new();
        let mut links = HashMap::new();
        let mut cards = String::new();
        let mut tree_items = String::new();
        let docs_ready = entries.iter().filter(|entry| entry.has_docs).count();

        for entry in entries {
            let entry_label = if entry.project_name.is_empty() {
                entry.project_code.clone()
            } else {
                format!("{} · {}", entry.project_code, entry.project_name)
            };
            graph_nodes.push(SollDocNode {
                id: entry.project_code.clone(),
                entity_type: "Project".to_string(),
                title: if entry.project_name.is_empty() {
                    entry.project_code.clone()
                } else {
                    format!("{} · {}", entry.project_code, entry.project_name)
                },
                description: entry.project_path.clone(),
                status: if entry.has_docs { "ready" } else { "pending" }.to_string(),
                metadata: "{}".to_string(),
            });
            graph_edges.push(SollDocEdge {
                source_id: "GLO".to_string(),
                target_id: entry.project_code.clone(),
                relation_type: "CONTAINS".to_string(),
            });
            if entry.has_docs {
                links.insert(
                    entry.project_code.clone(),
                    format!("{}/index.html", entry.project_code),
                );
            }
            cards.push_str(&format!(
                "<section class=\"card\"><h3>{}</h3><p class=\"muted\">{}</p><p><strong>Nodes:</strong> {}<br><strong>Status:</strong> {}</p>{}</section>",
                html_escape(&entry.project_code),
                html_escape(if entry.project_name.is_empty() { "Unnamed project" } else { &entry.project_name }),
                entry.node_count,
                if entry.has_docs { "docs ready" } else { "docs pending" },
                if entry.has_docs {
                    format!("<p><a href=\"{}/index.html\">Open project docs</a><br><span class=\"muted\">{}</span></p>", html_escape(&entry.project_code), html_escape(&entry.project_path))
                } else {
                    format!("<p class=\"muted\">No derived site yet.<br>{}</p>", html_escape(&entry.project_path))
                }
            ));
            if entry.has_docs {
                tree_items.push_str(&format!(
                    "<li class=\"tree-item leaf\"><a class=\"tree-link\" href=\"{}/index.html\"><span class=\"tree-tag\">PRJ</span><span>{}</span></a></li>",
                    html_escape(&entry.project_code),
                    html_escape(&entry_label)
                ));
            } else {
                tree_items.push_str(&format!(
                    "<li class=\"tree-item leaf\"><span class=\"tree-link muted\"><span class=\"tree-tag\">PRJ</span><span>{}</span></span></li>",
                    html_escape(&entry_label)
                ));
            }
        }

        let summary_html = format!(
            "<div class=\"cell\"><strong>Projects</strong><div>{}</div></div>\
             <div class=\"cell\"><strong>Docs Ready</strong><div>{}</div></div>\
             <div class=\"cell\"><strong>Scope</strong><div>all projects</div></div>\
             <div class=\"cell\"><strong>Boundary</strong><div>derived / non-canonical</div></div>",
            entries.len(),
            docs_ready,
        );

        let root_graph = render_mermaid_graph(&graph_nodes, &graph_edges, &links);
        let left_tree_html = format!(
            "<nav class=\"tree-shell\" aria-label=\"Portfolio tree\"><ul class=\"tree-root\">\
               <li class=\"tree-item branch root\"><details open>\
                 <summary><a class=\"tree-link current\" href=\"index.html\"><span class=\"tree-tag\">GLO</span><span>Global portfolio</span></a></summary>\
                 <ul class=\"tree-children\">{}</ul>\
               </details></li>\
             </ul></nav>",
            tree_items
        );
        render_site_page(
            "SOLL Derived Projects",
            "SOLL Derived Root",
            "Global human-readable index derived from live SOLL. This root is generated, incrementally refreshed when possible, and non-canonical.",
            "<span>GLO</span>",
            "Portfolio Tree",
            &left_tree_html,
            "Portfolio Focus",
            &root_graph,
            "Details",
            &cards,
            &summary_html,
        )
    }

    pub(crate) fn generate_soll_derived_docs(
        &self,
        project_code: &str,
        site_root: Option<&Path>,
        project_output_root: &Path,
    ) -> Result<SollDerivedDocsRefreshSummary, String> {
        if let Err(error) = self.resolve_canonical_project_identity_for_mutation(project_code) {
            return Err(format!("Invalid canonical project: {}", error));
        }

        let nodes = match self.load_soll_doc_nodes(project_code) {
            Ok(items) => items,
            Err(error) => return Err(format!("SOLL read error: {}", error)),
        };

        let edges = self
            .load_soll_doc_edges(project_code)
            .map_err(|error| format!("SOLL relation read error: {}", error))?;

        let generated_at_ms = now_unix_ms();
        let project_manifest_path = project_output_root.join("_manifest.json");
        let refresh_mode = if self.should_use_incremental_project_docs(&project_manifest_path) {
            "incremental"
        } else {
            if project_output_root.exists() {
                let _ = std::fs::remove_dir_all(project_output_root);
            }
            "full"
        };
        let pages = self.generate_soll_doc_pages(project_code, &nodes, &edges);
        let current_relative_paths = pages
            .iter()
            .map(|page| page.relative_path.clone())
            .collect::<HashSet<_>>();
        let deleted_paths = self.delete_obsolete_derived_doc_paths(
            &project_manifest_path,
            project_output_root,
            &current_relative_paths,
        )?;

        let mut pages_written = 0usize;
        let mut pages_unchanged = 0usize;
        let mut manifest_pages = Vec::new();
        let mut page_paths = Vec::new();
        for page in &pages {
            let page_path = project_output_root.join(&page.relative_path);
            match write_if_changed(&page_path, &page.html) {
                Ok(true) => pages_written += 1,
                Ok(false) => pages_unchanged += 1,
                Err(error) => {
                    return Err(format!("Derived docs write error: {}", error))
                }
            }
            manifest_pages.push(json!({
                "path": page.relative_path,
                "title": page.title,
                "content_hash": content_hash_hex(&page.html),
                "node_ids": page.node_ids,
                "edge_keys": page.edge_keys,
            }));
            page_paths.push(page_path.to_string_lossy().to_string());
        }

        let project_manifest = json!({
            "project_code": project_code,
            "generator_version": SOLL_PROJECT_DOCS_GENERATOR_VERSION,
            "refresh_mode": refresh_mode,
            "generated_at": generated_at_ms,
            "pages_total": pages.len(),
            "pages_written": pages_written,
            "pages_unchanged": pages_unchanged,
            "pages_deleted": deleted_paths.len(),
            "deleted_paths": deleted_paths,
            "pages": manifest_pages,
        });
        let project_manifest_pretty = serde_json::to_string_pretty(&project_manifest)
            .map_err(|error| format!("Manifest serialization error: {}", error))?;
        write_if_changed(&project_manifest_path, &project_manifest_pretty)
            .map_err(|error| format!("Manifest write error: {}", error))?;

        let (site_root_value, root_manifest_value, root_index_value, root_written) =
            if let Some(site_root) = site_root {
                let entries = self.load_soll_derived_project_entries(site_root);
                let root_index_path = site_root.join("index.html");
                let root_manifest_path = site_root.join("_root_manifest.json");
                let root_html = self.render_soll_root_page(&entries);
                let root_written = write_if_changed(&root_index_path, &root_html)
                    .map_err(|error| format!("Root index write error: {}", error))?;
                let root_manifest = json!({
                    "generator_version": SOLL_ROOT_DOCS_GENERATOR_VERSION,
                    "refresh_mode": refresh_mode,
                    "generated_at": generated_at_ms,
                    "projects_total": entries.len(),
                    "projects_with_docs": entries.iter().filter(|entry| entry.has_docs).count(),
                    "projects": entries.iter().map(|entry| json!({
                        "project_code": entry.project_code,
                        "project_name": entry.project_name,
                        "project_path": entry.project_path,
                        "node_count": entry.node_count,
                        "has_docs": entry.has_docs
                    })).collect::<Vec<_>>()
                });
                let root_manifest_pretty =
                    serde_json::to_string_pretty(&root_manifest).map_err(|error| {
                        format!("Root manifest serialization error: {}", error)
                    })?;
                write_if_changed(&root_manifest_path, &root_manifest_pretty)
                    .map_err(|error| format!("Root manifest write error: {}", error))?;
                (
                    site_root.to_string_lossy().to_string(),
                    root_manifest_path.to_string_lossy().to_string(),
                    root_index_path.to_string_lossy().to_string(),
                    root_written,
                )
            } else {
                (String::new(), String::new(), String::new(), false)
            };

        Ok(SollDerivedDocsRefreshSummary {
            project_code: project_code.to_string(),
            site_root: site_root_value,
            project_output_root: project_output_root.to_string_lossy().to_string(),
            project_manifest_path: project_manifest_path.to_string_lossy().to_string(),
            root_manifest_path: root_manifest_value,
            root_index_path: root_index_value,
            refresh_mode: refresh_mode.to_string(),
            pages_total: pages.len(),
            pages_written,
            pages_unchanged,
            pages_deleted: deleted_paths.len(),
            deleted_paths,
            root_written,
            stale_docs: false,
        })
    }

    pub(crate) fn axon_soll_generate_docs(&self, args: &Value) -> Option<Value> {
        let project_code = match args.get("project_code").and_then(|value| value.as_str()) {
            Some(value) if !value.trim().is_empty() => value.trim().to_ascii_uppercase(),
            _ => match self.validate_explicit_canonical_project_code(None, "soll_generate_docs") {
                Ok(code) => code,
                Err(e) => {
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                        "isError": true,
                        "data": {
                            "status": "input_invalid",
                            "parameter_repair": {
                                "invalid_field": "project_code",
                                "follow_up_tools": ["project_registry_lookup", "axon_init_project"],
                                "hint": "supply a registered `project_code`; call `project_registry_lookup` to list registered codes"
                            },
                            "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                        }
                    }))
                }
            },
        };

        let explicit_project_root = args.get("output_dir").and_then(|value| value.as_str());
        let explicit_site_root = args.get("site_root_dir").and_then(|value| value.as_str());
        let (site_root, project_output_root) = if let Some(site_root_dir) = explicit_site_root {
            let site_root = Path::new(site_root_dir).to_path_buf();
            (Some(site_root.clone()), site_root.join(&project_code))
        } else if let Some(project_root) = explicit_project_root {
            (None, Path::new(project_root).to_path_buf())
        } else {
            match canonical_soll_site_dir() {
                Some(path) => (Some(path.clone()), path.join(&project_code)),
                None => {
                    return Some(json!({
                        "content": [{ "type": "text", "text": "Cannot resolve canonical docs/derived/soll directory." }],
                        "isError": true,
                        "data": {
                            "status": "internal_error",
                            "parameter_repair": {
                                "invalid_field": "site_root_dir",
                                "follow_up_tools": ["status"],
                                "hint": "axon runtime cannot resolve docs/derived/soll directory; supply explicit `output_dir` or `site_root_dir`, or check `instance_identity.data_root_absolute` via `status mode=verbose`"
                            }
                        }
                    }))
                }
            }
        };

        match self.generate_soll_derived_docs(
            &project_code,
            site_root.as_deref(),
            &project_output_root,
        ) {
            Ok(summary) => Some(json!({
                "content": [{ "type": "text", "text": format!(
                    "Generated navigable SOLL docs for `{}`.\nSite root: {}\nProject root: {}\nRefresh mode: {}\nPages total: {}\nPages written: {}\nPages unchanged: {}\nPages deleted: {}\nProject manifest: {}\nRoot index: {}",
                    summary.project_code,
                    summary.site_root,
                    summary.project_output_root,
                    summary.refresh_mode,
                    summary.pages_total,
                    summary.pages_written,
                    summary.pages_unchanged,
                    summary.pages_deleted,
                    summary.project_manifest_path,
                    summary.root_index_path
                ) }],
                "data": {
                    "project_code": summary.project_code,
                    "site_root": json_optional_string(&summary.site_root),
                    "output_root": summary.project_output_root,
                    "manifest_path": summary.project_manifest_path,
                    "root_manifest_path": json_optional_string(&summary.root_manifest_path),
                    "root_index_path": json_optional_string(&summary.root_index_path),
                    "refresh_mode": summary.refresh_mode,
                    "pages_total": summary.pages_total,
                    "pages_written": summary.pages_written,
                    "pages_unchanged": summary.pages_unchanged,
                    "pages_deleted": summary.pages_deleted,
                    "deleted_paths": summary.deleted_paths,
                    "root_written": summary.root_written,
                    "stale_docs": summary.stale_docs,
                    "canonical_boundary": "Derived human docs only. Live SOLL and SOLL_EXPORT remain canonical."
                }
            })),
            Err(error) => Some(json!({
                "content": [{ "type": "text", "text": error.clone() }],
                "isError": true,
                "data": {
                    "status": "internal_error",
                    "parameter_repair": {
                        "invalid_field": "site_root_dir|output_dir",
                        "follow_up_tools": ["status", "soll_validate"],
                        "hint": "doc generation failed; verify SOLL state via `soll_validate` and the runtime is healthy via `status`. Filesystem write errors usually indicate permission or disk-space issues at the supplied output_dir/site_root_dir"
                    },
                    "diagnostic_excerpt": error.chars().take(240).collect::<String>()
                }
            })),
        }
    }
}
