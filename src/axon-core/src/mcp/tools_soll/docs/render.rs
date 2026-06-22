use super::hierarchy::{entity_type_short_label, html_escape};
use super::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

pub(super) fn json_optional_string(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        json!(value)
    }
}

fn mermaid_escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "&quot;")
        .replace('\n', "<br/>")
}

fn summarize_for_label(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut summary = trimmed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    summary.push('…');
    summary
}

pub(super) fn content_hash_hex(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(super) fn write_if_changed(path: &Path, content: &str) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == content {
            return Ok(false);
        }
    }

    std::fs::write(path, content)?;
    Ok(true)
}

pub(super) struct RenderedMermaidGraph {
    pub(super) definition: String,
    pub(super) link_map_json: String,
}

/// REQ-AXO-312 — Hierarchy Focus partition for the local (level ±1) graph.
/// Drives a strict macro→micro left-to-right layout: `macro_ids` (parents /
/// upstream / containing roots) land in the left column, the focus node in the
/// middle, `micro_ids` (children / downstream) in the right column. Any local
/// node not listed falls back into the macro column as ambient context.
pub(super) struct MermaidFocus {
    pub(super) focus_id: String,
    pub(super) macro_ids: HashSet<String>,
    pub(super) micro_ids: HashSet<String>,
}

fn mermaid_node_decl(
    indent: &str,
    node: &SollDocNode,
    mermaid_ids: &HashMap<String, String>,
) -> String {
    let label = format!(
        "{} {}: {}",
        entity_type_short_label(&node.entity_type),
        node.id,
        summarize_for_label(&node.title, 42)
    );
    format!(
        "{}{}[\"{}\"]\n",
        indent,
        mermaid_ids
            .get(&node.id)
            .map(String::as_str)
            .unwrap_or("NODE"),
        mermaid_escape_label(&label)
    )
}

pub(super) fn render_mermaid_graph(
    nodes: &[SollDocNode],
    edges: &[SollDocEdge],
    links: &HashMap<String, String>,
    focus: Option<&MermaidFocus>,
) -> RenderedMermaidGraph {
    let mut ordered_nodes = nodes.to_vec();
    ordered_nodes.sort_by(|left, right| left.id.cmp(&right.id));
    let mermaid_ids = ordered_nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| (node.id.clone(), format!("N{}", idx)))
        .collect::<HashMap<_, _>>();

    let mut ordered_edges = edges.to_vec();
    ordered_edges.sort_by(|left, right| {
        (&left.source_id, &left.relation_type, &left.target_id).cmp(&(
            &right.source_id,
            &right.relation_type,
            &right.target_id,
        ))
    });

    let mut graph = String::from("flowchart LR\n");
    match focus {
        // REQ-AXO-312 — three-column macro → focus → micro layout. Subgraphs
        // are declared left-to-right in `flowchart LR`, so declaration order
        // fixes the columns; `direction TB` stacks each column vertically.
        Some(focus) => {
            let mut macro_nodes = Vec::new();
            let mut focus_nodes = Vec::new();
            let mut micro_nodes = Vec::new();
            for node in &ordered_nodes {
                if node.id == focus.focus_id {
                    focus_nodes.push(node);
                } else if focus.micro_ids.contains(&node.id) {
                    micro_nodes.push(node);
                } else if focus.macro_ids.contains(&node.id) {
                    macro_nodes.push(node);
                } else {
                    // Any ambient local node not classified as a child reads as
                    // "macro" relative to the focus, so it lands in the left
                    // column rather than disappearing.
                    macro_nodes.push(node);
                }
            }
            let push_subgraph = |graph: &mut String, sg_id: &str, title: &str, bucket: &[&SollDocNode]| {
                if bucket.is_empty() {
                    return;
                }
                graph.push_str(&format!("  subgraph {}[\"{}\"]\n", sg_id, title));
                graph.push_str("    direction TB\n");
                for node in bucket {
                    graph.push_str(&mermaid_node_decl("    ", node, &mermaid_ids));
                }
                graph.push_str("  end\n");
            };
            push_subgraph(&mut graph, "sgMacro", "▲ Macro · niveau −1", &macro_nodes);
            push_subgraph(&mut graph, "sgFocus", "● Focus", &focus_nodes);
            push_subgraph(&mut graph, "sgMicro", "▼ Micro · niveau +1", &micro_nodes);
        }
        None => {
            for node in &ordered_nodes {
                graph.push_str(&mermaid_node_decl("  ", node, &mermaid_ids));
            }
        }
    }
    // REQ-AXO-312 — column rank for the focus layout: macro=0, focus=1,
    // micro=2 (default 0). Edges are emitted from the lower rank to the higher
    // rank so dagre flows them left→right, pinning macro left and micro right
    // regardless of the stored (child→parent) SOLL direction. The relation
    // label is preserved; exact relation direction stays in the right-panel
    // diagnostics. Without a focus every rank is 0, so order is untouched.
    let column_rank = |canonical_id: &str| -> u8 {
        match focus {
            Some(focus) if focus.focus_id == canonical_id => 1,
            Some(focus) if focus.micro_ids.contains(canonical_id) => 2,
            Some(_) => 0,
            None => 0,
        }
    };
    for edge in ordered_edges {
        let (head_canonical, tail_canonical) =
            if column_rank(&edge.source_id) > column_rank(&edge.target_id) {
                (&edge.target_id, &edge.source_id)
            } else {
                (&edge.source_id, &edge.target_id)
            };
        let head_id = mermaid_ids
            .get(head_canonical)
            .map(String::as_str)
            .unwrap_or("NODE");
        let tail_id = mermaid_ids
            .get(tail_canonical)
            .map(String::as_str)
            .unwrap_or("NODE");
        graph.push_str(&format!(
            "  {} -- {} --> {}\n",
            head_id,
            mermaid_escape_label(&edge.relation_type),
            tail_id
        ));
    }

    let mut link_pairs = links.iter().collect::<Vec<_>>();
    link_pairs.sort_by(|left, right| left.0.cmp(right.0));
    for (canonical_node_id, href) in link_pairs {
        let Some(mermaid_id) = mermaid_ids.get(canonical_node_id) else {
            continue;
        };
        graph.push_str(&format!(
            "  click {} href \"{}\" \"Open {}\"\n",
            mermaid_id,
            href,
            mermaid_escape_label(canonical_node_id)
        ));
    }

    let link_map_json = serde_json::to_string(
        &mermaid_ids
            .iter()
            .filter_map(|(canonical_id, mermaid_id)| {
                links
                    .get(canonical_id)
                    .map(|href| (mermaid_id.clone(), href.clone()))
            })
            .collect::<BTreeMap<_, _>>(),
    )
    .unwrap_or_else(|_| "{}".to_string());

    RenderedMermaidGraph {
        definition: graph,
        link_map_json,
    }
}

pub(super) fn render_site_page(
    page_title: &str,
    eyebrow: &str,
    intro: &str,
    breadcrumb_html: &str,
    left_title: &str,
    left_panel_html: &str,
    center_title: &str,
    graph: &RenderedMermaidGraph,
    right_title: &str,
    right_panel_html: &str,
    summary_html: &str,
) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{page_title}</title>
  <script src="https://cdn.jsdelivr.net/npm/mermaid@10/dist/mermaid.min.js"></script>
  <style>
    :root {{
      --bg: #f5f1e8;
      --surface: rgba(255,255,255,0.92);
      --border: rgba(64, 49, 21, 0.14);
      --text: #22170d;
      --muted: #6f5f49;
      --accent: #1f7a6b;
      --accent-2: #b55c2f;
      --shadow: 0 20px 60px rgba(48, 34, 12, 0.12);
      --radius: 22px;
      --left-pane-width: 300px;
      --right-pane-width: 360px;
      --handle-width: 12px;
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      font-family: "Space Grotesk", system-ui, sans-serif;
      background:
        radial-gradient(circle at top right, rgba(31,122,107,0.14), transparent 20%),
        radial-gradient(circle at top left, rgba(181,92,47,0.14), transparent 22%),
        var(--bg);
      color: var(--text);
    }}
    .page {{ width: calc(100vw - 24px); margin: 12px auto 24px; }}
    .hero, .card {{
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: var(--radius);
      box-shadow: var(--shadow);
    }}
    .hero {{ padding: 24px 26px; }}
    .eyebrow {{
      font-size: 12px;
      font-weight: 700;
      letter-spacing: 0.12em;
      text-transform: uppercase;
      color: var(--accent);
      margin-bottom: 10px;
    }}
    h1 {{ margin: 0 0 10px; font-size: clamp(2rem, 4vw, 3.6rem); line-height: 0.95; }}
    .lede {{ margin: 0; color: var(--muted); max-width: 70ch; }}
    .breadcrumb {{
      margin: 14px 0 0;
      display: flex;
      flex-wrap: wrap;
      gap: 10px;
      font-size: 0.94rem;
      color: var(--muted);
    }}
    .breadcrumb a {{ color: var(--accent); text-decoration: none; }}
    .toolbar {{
      display: flex;
      flex-wrap: wrap;
      gap: 10px;
      margin-top: 20px;
    }}
    .toolbar button {{
      border: 1px solid var(--border);
      border-radius: 999px;
      background: rgba(255,255,255,0.78);
      padding: 10px 14px;
      font: inherit;
      color: var(--text);
      cursor: pointer;
    }}
    .workspace {{
      display: grid;
      grid-template-columns: var(--left-pane-width) var(--handle-width) minmax(0, 1fr) var(--handle-width) var(--right-pane-width);
      gap: 0;
      align-items: stretch;
      min-height: calc(100vh - 220px);
      margin-top: 18px;
    }}
    /* REQ-AXO-313 — symmetric space redistribution across tree / graph /
       details. Every collapse combination keeps the surviving panes sharing
       the freed space; columns are [left][h][center][h][right]. */
    body.right-collapsed .workspace {{
      grid-template-columns: var(--left-pane-width) var(--handle-width) minmax(0, 1fr) 0px 0px;
    }}
    body.left-collapsed .workspace {{
      grid-template-columns: 0px 0px minmax(0, 1fr) var(--handle-width) var(--right-pane-width);
    }}
    body.center-collapsed .workspace {{
      grid-template-columns: minmax(0, 1fr) 0px 0px 0px minmax(0, 1fr);
    }}
    body.left-collapsed.right-collapsed .workspace {{
      grid-template-columns: 0px 0px minmax(0, 1fr) 0px 0px;
    }}
    body.center-collapsed.right-collapsed .workspace {{
      grid-template-columns: minmax(0, 1fr) 0px 0px 0px 0px;
    }}
    body.left-collapsed.center-collapsed .workspace {{
      grid-template-columns: 0px 0px 0px 0px minmax(0, 1fr);
    }}
    .pane, .center-pane {{
      min-width: 0;
    }}
    .pane-inner, .center-pane {{
      height: 100%;
      overflow: auto;
      padding: 18px;
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: var(--radius);
      box-shadow: var(--shadow);
    }}
    .center-pane h2, .pane h2 {{ margin-top: 0; }}
    .resize-handle {{
      position: relative;
      width: var(--handle-width);
      cursor: col-resize;
      background: transparent;
    }}
    .resize-handle::before {{
      content: "";
      position: absolute;
      top: 14px;
      bottom: 14px;
      left: calc(50% - 1px);
      width: 2px;
      border-radius: 999px;
      background: rgba(64, 49, 21, 0.16);
    }}
    /* A resize handle is meaningful only between a visible side pane and the
       visible flexible center; in any center-collapsed split the two side
       panes share space 50/50 with no draggable seam. Use visibility (not
       display) so the handle keeps its grid cell — display:none would drop it
       as a grid item and shift every following pane into the wrong column. */
    body.left-collapsed .resize-left,
    body.center-collapsed .resize-left,
    body.right-collapsed .resize-right,
    body.center-collapsed .resize-right {{
      visibility: hidden;
    }}
    /* A collapsed pane keeps its (zero-width) grid cell but renders nothing,
       so its padding box never bleeds over the surviving neighbour. */
    body.left-collapsed .pane-left,
    body.center-collapsed .center-pane,
    body.right-collapsed .pane-right {{
      visibility: hidden;
    }}
    .card {{ padding: 18px 18px 16px; }}
    .card h2, .card h3 {{ margin-top: 0; }}
    .mermaid {{
      background: rgba(255,255,255,0.62);
      border-radius: 16px;
      padding: 8px;
      overflow: auto;
    }}
    .node-list, .relation-list {{
      margin: 0;
      padding-left: 18px;
    }}
    .node-list li, .relation-list li {{ margin: 8px 0; }}
    a {{ color: var(--accent-2); }}
    code {{
      background: rgba(31,122,107,0.08);
      border-radius: 8px;
      padding: 0 6px;
      font-size: 0.92em;
    }}
    .muted {{ color: var(--muted); }}
    .rel {{ font-weight: 700; color: var(--accent); }}
    .summary-grid {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
      gap: 12px;
      margin-top: 14px;
    }}
    .summary-grid .cell {{
      padding: 12px 14px;
      border: 1px solid var(--border);
      border-radius: 14px;
      background: rgba(181,92,47,0.05);
    }}
    .tree-shell, .tree-root, .tree-children {{
      margin: 0;
      padding-left: 0;
      list-style: none;
    }}
    .tree-item {{ margin: 4px 0; }}
    .tree-item > details > summary {{
      list-style: none;
      cursor: pointer;
    }}
    .tree-item > details > summary::-webkit-details-marker {{ display: none; }}
    .tree-link {{
      display: inline-flex;
      align-items: center;
      gap: 8px;
      width: calc(100% - 18px);
      padding: 8px 10px;
      border-radius: 12px;
      text-decoration: none;
      color: var(--text);
    }}
    .tree-link:hover {{ background: rgba(31,122,107,0.08); }}
    .tree-link.current {{
      background: rgba(31,122,107,0.14);
      font-weight: 700;
    }}
    .tree-tag {{
      display: inline-flex;
      align-items: center;
      justify-content: center;
      min-width: 42px;
      padding: 4px 8px;
      border-radius: 999px;
      background: rgba(181,92,47,0.12);
      font-size: 0.76rem;
      letter-spacing: 0.04em;
      text-transform: uppercase;
      color: var(--accent-2);
    }}
    .tree-children {{
      margin-left: 18px;
      padding-left: 12px;
      border-left: 1px dashed rgba(64, 49, 21, 0.18);
    }}
    .panel-meta {{
      margin: 0 0 12px;
      color: var(--muted);
      font-size: 0.95rem;
    }}
    @media (max-width: 960px) {{
      .page {{ width: calc(100vw - 12px); }}
      .workspace {{
        grid-template-columns: 1fr;
        min-height: auto;
      }}
      .resize-handle {{ display: none; }}
      body[class*="collapsed"] .workspace {{
        grid-template-columns: 1fr;
      }}
    }}
  </style>
</head>
<body>
  <div class="page">
    <section class="hero">
      <div class="eyebrow">{eyebrow}</div>
      <h1>{page_title}</h1>
      <p class="lede">{intro}</p>
      <div class="breadcrumb">{breadcrumb_html}</div>
      <div class="summary-grid">{summary_html}</div>
    </section>
    <section class="toolbar" aria-label="Pane controls">
      <button id="toggle-left" type="button" aria-expanded="true" aria-controls="left-pane">Toggle tree</button>
      <button id="toggle-graph" type="button" aria-expanded="true" aria-controls="center-pane">Toggle graph</button>
      <button id="toggle-right" type="button" aria-expanded="true" aria-controls="right-pane">Toggle details</button>
    </section>
    <section class="workspace">
      <aside class="pane pane-left" id="left-pane" aria-label="Hierarchy tree">
        <div class="pane-inner">
          <h2>{left_title}</h2>
          <p class="panel-meta">Primary navigation path. HTML links and tree links are canonical; Mermaid clicks are enhancement only.</p>
          {left_panel_html}
        </div>
      </aside>
      <div class="resize-handle resize-left" data-side="left" aria-hidden="true"></div>
      <article class="center-pane" id="center-pane" aria-label="Graph">
        <h2>{center_title}</h2>
        <div class="mermaid" data-link-map='{graph_link_map_json}'>
{graph_definition}
        </div>
        <details>
          <summary>Graph source</summary>
          <pre>{graph_source_html}</pre>
        </details>
      </article>
      <div class="resize-handle resize-right" data-side="right" aria-hidden="true"></div>
      <aside class="pane pane-right" id="right-pane" aria-label="Details">
        <div class="pane-inner">
          <h2>{right_title}</h2>
          {right_panel_html}
        </div>
      </aside>
    </section>
  </div>
  <script>
    mermaid.initialize({{
      startOnLoad: true,
      securityLevel: "loose",
      theme: "base",
      flowchart: {{
        useMaxWidth: true,
        htmlLabels: true
      }},
      themeVariables: {{
        primaryColor: "#eae3d3",
        primaryTextColor: "#22170d",
        primaryBorderColor: "#83633f",
        lineColor: "#4f6f67",
        tertiaryColor: "#f7f3eb"
      }}
    }});

    function safeStorage() {{
      try {{
        const key = "__axon_docs_probe__";
        window.localStorage.setItem(key, "1");
        window.localStorage.removeItem(key);
        return window.localStorage;
      }} catch (_error) {{
        return null;
      }}
    }}

    const storage = safeStorage();

    const PANES = [
      {{ side: "left", cls: "left-collapsed", key: "axon-docs-left-collapsed", button: "toggle-left" }},
      {{ side: "center", cls: "center-collapsed", key: "axon-docs-center-collapsed", button: "toggle-graph" }},
      {{ side: "right", cls: "right-collapsed", key: "axon-docs-right-collapsed", button: "toggle-right" }},
    ];

    function visiblePaneCount() {{
      return PANES.filter((pane) => !document.body.classList.contains(pane.cls)).length;
    }}

    function syncPaneButton(pane) {{
      const button = document.getElementById(pane.button);
      if (button) {{
        button.setAttribute("aria-expanded", String(!document.body.classList.contains(pane.cls)));
      }}
    }}

    function applyPaneState() {{
      if (!storage) {{
        return;
      }}
      const leftWidth = storage.getItem("axon-docs-left-width");
      const rightWidth = storage.getItem("axon-docs-right-width");
      if (leftWidth) {{
        document.documentElement.style.setProperty("--left-pane-width", leftWidth);
      }}
      if (rightWidth) {{
        document.documentElement.style.setProperty("--right-pane-width", rightWidth);
      }}
      PANES.forEach((pane) => {{
        document.body.classList.toggle(pane.cls, storage.getItem(pane.key) === "1");
      }});
      // Never restore a fully-collapsed workspace — keep at least the graph.
      if (visiblePaneCount() === 0) {{
        document.body.classList.remove("center-collapsed");
      }}
      PANES.forEach(syncPaneButton);
    }}

    function persistPaneState() {{
      if (!storage) {{
        return;
      }}
      PANES.forEach((pane) => {{
        storage.setItem(pane.key, document.body.classList.contains(pane.cls) ? "1" : "0");
      }});
      storage.setItem("axon-docs-left-width", getComputedStyle(document.documentElement).getPropertyValue("--left-pane-width").trim() || "300px");
      storage.setItem("axon-docs-right-width", getComputedStyle(document.documentElement).getPropertyValue("--right-pane-width").trim() || "360px");
    }}

    function togglePane(side) {{
      const pane = PANES.find((entry) => entry.side === side);
      if (!pane) {{
        return;
      }}
      const willCollapse = !document.body.classList.contains(pane.cls);
      // Refuse to hide the last visible pane — at least one must stay open.
      if (willCollapse && visiblePaneCount() <= 1) {{
        return;
      }}
      document.body.classList.toggle(pane.cls);
      syncPaneButton(pane);
      persistPaneState();
    }}

    function installPaneControls() {{
      PANES.forEach((pane) => {{
        const button = document.getElementById(pane.button);
        if (button) {{
          button.addEventListener("click", () => togglePane(pane.side));
        }}
      }});

      document.querySelectorAll(".resize-handle[data-side]").forEach((handle) => {{
        handle.addEventListener("pointerdown", (event) => {{
          const side = handle.dataset.side;
          const startX = event.clientX;
          const startLeft = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--left-pane-width")) || 300;
          const startRight = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--right-pane-width")) || 360;
          const onMove = (moveEvent) => {{
            if (side === "left") {{
              const next = Math.max(180, Math.min(520, startLeft + (moveEvent.clientX - startX)));
              document.documentElement.style.setProperty("--left-pane-width", `${{next}}px`);
            }} else {{
              const next = Math.max(220, Math.min(620, startRight - (moveEvent.clientX - startX)));
              document.documentElement.style.setProperty("--right-pane-width", `${{next}}px`);
            }}
          }};
          const onUp = () => {{
            window.removeEventListener("pointermove", onMove);
            window.removeEventListener("pointerup", onUp);
            persistPaneState();
          }};
          window.addEventListener("pointermove", onMove);
          window.addEventListener("pointerup", onUp);
        }});
      }});
    }}

    function bindMermaidNodeLinks() {{
      document.querySelectorAll('.mermaid[data-link-map]').forEach((container) => {{
        let linkMap = {{}};
        try {{
          linkMap = JSON.parse(container.dataset.linkMap || '{{}}');
        }} catch (_error) {{
          linkMap = {{}};
        }}
        const svg = container.querySelector('svg');
        if (!svg) {{
          return;
        }}
        Object.entries(linkMap).forEach(([nodeId, href]) => {{
          const node = svg.querySelector(`g.node[id*="flowchart-${{nodeId}}-"]`);
          if (!node || node.dataset.axonBound === '1') {{
            return;
          }}
          node.dataset.axonBound = '1';
          node.style.cursor = 'pointer';
          node.addEventListener('click', () => {{
            window.location.href = href;
          }});
        }});
      }});
    }}

    window.addEventListener('load', () => {{
      applyPaneState();
      installPaneControls();
      let attempts = 0;
      const timer = window.setInterval(() => {{
        bindMermaidNodeLinks();
        attempts += 1;
        if (document.querySelector('.mermaid svg') || attempts >= 20) {{
          window.clearInterval(timer);
        }}
      }}, 150);
    }});
  </script>
</body>
</html>
"##,
        page_title = html_escape(page_title),
        eyebrow = html_escape(eyebrow),
        intro = html_escape(intro),
        breadcrumb_html = breadcrumb_html,
        left_title = html_escape(left_title),
        left_panel_html = left_panel_html,
        center_title = html_escape(center_title),
        summary_html = summary_html,
        graph_definition = graph.definition,
        graph_link_map_json = html_escape(&graph.link_map_json),
        graph_source_html = html_escape(&graph.definition),
        right_title = html_escape(right_title),
        right_panel_html = right_panel_html,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: &str, title: &str) -> SollDocNode {
        SollDocNode {
            id: id.to_string(),
            entity_type: kind.to_string(),
            title: title.to_string(),
            description: format!("Description for {}", id),
            status: "current".to_string(),
            metadata: "{}".to_string(),
        }
    }

    fn edge(src: &str, rel: &str, tgt: &str) -> SollDocEdge {
        SollDocEdge {
            source_id: src.to_string(),
            target_id: tgt.to_string(),
            relation_type: rel.to_string(),
        }
    }

    // REQ-AXO-312 — the focus layout must wrap nodes in macro / focus / micro
    // subgraphs and reorient a stored child→parent edge so the parent (macro)
    // becomes the edge head, pinning macro left of micro regardless of the
    // SOLL direction. Without a focus the edge keeps its stored direction.
    #[test]
    fn focus_layout_builds_columns_and_reorients_edges() {
        // ids chosen so sorted order is deterministic: AAA=N0, ZZZ=N1.
        let nodes = vec![
            node("AAA-AXO-001", "Milestone", "macro parent"),
            node("ZZZ-AXO-009", "Requirement", "focus node"),
        ];
        // Stored direction is child→parent: focus REFINES the macro parent.
        let edges = vec![edge("ZZZ-AXO-009", "REFINES", "AAA-AXO-001")];
        let links = HashMap::new();
        let focus = MermaidFocus {
            focus_id: "ZZZ-AXO-009".to_string(),
            macro_ids: ["AAA-AXO-001".to_string()].into_iter().collect(),
            micro_ids: HashSet::new(),
        };

        let focused = render_mermaid_graph(&nodes, &edges, &links, Some(&focus)).definition;
        assert!(focused.contains("subgraph sgMacro"), "{focused}");
        assert!(focused.contains("subgraph sgFocus"), "{focused}");
        assert!(focused.contains("▲ Macro"), "{focused}");
        // Reoriented: macro (N0) is the head, focus (N1) the tail.
        assert!(focused.contains("N0 -- REFINES --> N1"), "{focused}");

        let flat = render_mermaid_graph(&nodes, &edges, &links, None).definition;
        assert!(!flat.contains("subgraph"), "{flat}");
        // Untouched: stored direction focus (N1) → macro (N0).
        assert!(flat.contains("N1 -- REFINES --> N0"), "{flat}");
    }

    // REQ-AXO-313 — three independent toggles + symmetric grid collapse rules.
    #[test]
    fn site_page_exposes_three_toggles_and_symmetric_grid() {
        let graph = render_mermaid_graph(&[], &[], &HashMap::new(), None);
        let page = render_site_page(
            "t", "e", "i", "b", "Tree", "tree", "Graph", &graph, "Details", "details", "s",
        );
        assert!(page.contains("id=\"toggle-left\""));
        assert!(page.contains("id=\"toggle-graph\""));
        assert!(page.contains("id=\"toggle-right\""));
        assert!(page.contains("id=\"center-pane\""));
        assert!(page.contains("body.center-collapsed .workspace"));
        assert!(page.contains("axon-docs-center-collapsed"));
        assert!(page.contains("visiblePaneCount"));
    }

    // REQ-AXO-312 / 313 visual preview. Run with AXON_AUTODOC_PREVIEW=1 to also
    // dump representative pages to /tmp/axon-autodoc-preview/ for browser
    // iteration; otherwise it only exercises the render path:
    //   AXON_AUTODOC_PREVIEW=1 cargo test -p axon-core --lib \
    //     render::tests::render_autodoc_preview -- --nocapture
    #[test]
    fn render_autodoc_preview() {
        let nodes = vec![
            node("MIL-AXO-040", "Milestone", "Complétude indexeur : call-graph complet"),
            node("DEC-AXO-060", "Decision", "4 verbes canoniques runtime"),
            node("REQ-AXO-100", "Requirement", "Hierarchy Focus macro→micro layout"),
            node("REQ-AXO-101", "Requirement", "Toggle tree/graph/detail symétrique"),
            node("REQ-AXO-102", "Requirement", "Subgraph LR colonnes"),
            node("REQ-AXO-103", "Requirement", "Click-through mermaid nodes"),
            node("CPT-AXO-054", "Concept", "Streaming pipeline v2"),
            node("VAL-AXO-009", "Validation", "Preuve navigateur autodoc"),
        ];
        let edges = vec![
            edge("REQ-AXO-100", "BELONGS_TO", "MIL-AXO-040"),
            edge("REQ-AXO-100", "REFINES", "DEC-AXO-060"),
            edge("REQ-AXO-101", "REFINES", "REQ-AXO-100"),
            edge("REQ-AXO-102", "REFINES", "REQ-AXO-100"),
            edge("REQ-AXO-103", "REFINES", "REQ-AXO-100"),
            edge("CPT-AXO-054", "EXPLAINS", "REQ-AXO-100"),
            edge("VAL-AXO-009", "VERIFIES", "REQ-AXO-100"),
        ];
        let links = nodes
            .iter()
            .map(|n| (n.id.clone(), node_file_name(&n.id)))
            .collect::<HashMap<_, _>>();

        let macro_ids = ["MIL-AXO-040", "DEC-AXO-060", "CPT-AXO-054"]
            .iter()
            .map(|s| s.to_string())
            .collect::<HashSet<_>>();
        let micro_ids = ["REQ-AXO-101", "REQ-AXO-102", "REQ-AXO-103", "VAL-AXO-009"]
            .iter()
            .map(|s| s.to_string())
            .collect::<HashSet<_>>();
        let focus = MermaidFocus {
            focus_id: "REQ-AXO-100".to_string(),
            macro_ids,
            micro_ids,
        };

        let focused = render_mermaid_graph(&nodes, &edges, &links, Some(&focus));
        let flat = render_mermaid_graph(&nodes, &edges, &links, None);

        let node_page = render_site_page(
            "REQ-AXO-100 · Hierarchy Focus macro→micro layout",
            "SOLL Derived Node (preview)",
            "Aperçu navigateur REQ-AXO-312 / 313 : graphe local niveau ±1 macro→micro + toggles tree/graph/detail.",
            "<a href=\"#\">GLO</a><span>/</span><a href=\"#\">AXO</a><span>/</span><span>REQ-AXO-100</span>",
            "Project Tree",
            "<nav class=\"tree-shell\"><ul class=\"tree-root\"><li class=\"tree-item\"><a class=\"tree-link current\"><span class=\"tree-tag\">REQ</span><span>REQ-AXO-100</span></a></li></ul></nav>",
            "Local Graph",
            &focused,
            "Details",
            "<section class=\"card\"><h3>Description</h3><p>Aperçu de la mise en page macro→micro.</p></section><section class=\"card\"><h3>Incoming Neighbors</h3><ul class=\"node-list\"><li>CPT-AXO-054</li></ul></section>",
            "<div class=\"cell\"><strong>Kind</strong><div>Requirement</div></div><div class=\"cell\"><strong>Relations</strong><div>7</div></div>",
        );

        let flat_page = render_site_page(
            "Flat graph (focus-less) preview",
            "SOLL Derived Root (preview)",
            "Aperçu du rendu plat sans focus (pages projet / racine).",
            "<span>GLO</span>",
            "Portfolio Tree",
            "<nav class=\"tree-shell\"><ul class=\"tree-root\"><li class=\"tree-item\"><span class=\"tree-link\">GLO</span></li></ul></nav>",
            "Portfolio Focus",
            &flat,
            "Details",
            "<section class=\"card\"><h3>Reading Model</h3><p>Rendu plat.</p></section>",
            "<div class=\"cell\"><strong>Mode</strong><div>flat</div></div>",
        );

        // Both render paths must at least produce the focus + flat surfaces.
        assert!(node_page.contains("subgraph sgMacro"));
        assert!(flat_page.contains("flowchart LR"));

        if std::env::var("AXON_AUTODOC_PREVIEW").is_ok() {
            let dir = std::path::Path::new("/tmp/axon-autodoc-preview");
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("node-preview.html"), node_page).unwrap();
            std::fs::write(dir.join("flat-preview.html"), flat_page).unwrap();
            println!("autodoc preview written to {}", dir.display());
        }
    }
}
