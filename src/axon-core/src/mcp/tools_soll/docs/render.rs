use super::hierarchy::{entity_type_short_label, html_escape};
use super::*;
use std::collections::{BTreeMap, HashMap};
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

pub(super) fn render_mermaid_graph(
    nodes: &[SollDocNode],
    edges: &[SollDocEdge],
    links: &HashMap<String, String>,
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
    for node in ordered_nodes {
        let label = format!(
            "{} {}: {}",
            entity_type_short_label(&node.entity_type),
            node.id,
            summarize_for_label(&node.title, 42)
        );
        graph.push_str(&format!(
            "  {}[\"{}\"]\n",
            mermaid_ids
                .get(&node.id)
                .map(String::as_str)
                .unwrap_or("NODE"),
            mermaid_escape_label(&label)
        ));
    }
    for edge in ordered_edges {
        let source_id = mermaid_ids
            .get(&edge.source_id)
            .map(String::as_str)
            .unwrap_or("NODE");
        let target_id = mermaid_ids
            .get(&edge.target_id)
            .map(String::as_str)
            .unwrap_or("NODE");
        graph.push_str(&format!(
            "  {} -- {} --> {}\n",
            source_id,
            mermaid_escape_label(&edge.relation_type),
            target_id
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
    body.left-collapsed .workspace {{
      grid-template-columns: 0px 0px minmax(0, 1fr) var(--handle-width) var(--right-pane-width);
    }}
    body.right-collapsed .workspace {{
      grid-template-columns: var(--left-pane-width) var(--handle-width) minmax(0, 1fr) 0px 0px;
    }}
    body.left-collapsed.right-collapsed .workspace {{
      grid-template-columns: 0px 0px minmax(0, 1fr) 0px 0px;
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
    body.left-collapsed .resize-left,
    body.right-collapsed .resize-right {{
      display: none;
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
      body.left-collapsed .workspace,
      body.right-collapsed .workspace,
      body.left-collapsed.right-collapsed .workspace {{
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
      <article class="center-pane">
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

    function applyPaneState() {{
      if (!storage) {{
        return;
      }}
      const leftWidth = storage.getItem("axon-docs-left-width");
      const rightWidth = storage.getItem("axon-docs-right-width");
      const leftCollapsed = storage.getItem("axon-docs-left-collapsed") === "1";
      const rightCollapsed = storage.getItem("axon-docs-right-collapsed") === "1";
      if (leftWidth) {{
        document.documentElement.style.setProperty("--left-pane-width", leftWidth);
      }}
      if (rightWidth) {{
        document.documentElement.style.setProperty("--right-pane-width", rightWidth);
      }}
      document.body.classList.toggle("left-collapsed", leftCollapsed);
      document.body.classList.toggle("right-collapsed", rightCollapsed);
      const leftButton = document.getElementById("toggle-left");
      const rightButton = document.getElementById("toggle-right");
      if (leftButton) {{
        leftButton.setAttribute("aria-expanded", String(!leftCollapsed));
      }}
      if (rightButton) {{
        rightButton.setAttribute("aria-expanded", String(!rightCollapsed));
      }}
    }}

    function persistPaneState() {{
      if (!storage) {{
        return;
      }}
      storage.setItem("axon-docs-left-collapsed", document.body.classList.contains("left-collapsed") ? "1" : "0");
      storage.setItem("axon-docs-right-collapsed", document.body.classList.contains("right-collapsed") ? "1" : "0");
      storage.setItem("axon-docs-left-width", getComputedStyle(document.documentElement).getPropertyValue("--left-pane-width").trim() || "300px");
      storage.setItem("axon-docs-right-width", getComputedStyle(document.documentElement).getPropertyValue("--right-pane-width").trim() || "360px");
    }}

    function togglePane(side) {{
      const className = side === "left" ? "left-collapsed" : "right-collapsed";
      document.body.classList.toggle(className);
      const button = document.getElementById(side === "left" ? "toggle-left" : "toggle-right");
      if (button) {{
        button.setAttribute("aria-expanded", String(!document.body.classList.contains(className)));
      }}
      persistPaneState();
    }}

    function installPaneControls() {{
      const leftButton = document.getElementById("toggle-left");
      const rightButton = document.getElementById("toggle-right");
      if (leftButton) {{
        leftButton.addEventListener("click", () => togglePane("left"));
      }}
      if (rightButton) {{
        rightButton.addEventListener("click", () => togglePane("right"));
      }}

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
