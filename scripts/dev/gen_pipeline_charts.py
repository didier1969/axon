#!/usr/bin/env python3
"""REQ session-71 — generate 3 self-contained HTML pipeline charts from the
MCP-research JSON specs (one per doc). Mermaid via CDN; everything else inline.
Ideal / Actuel / Delta+criticité / Targets / Couches macro-meso-micro / Faits."""
import json, html, glob, os, sys

SRC = sys.argv[1] if len(sys.argv) > 1 else "/tmp/axon-docs"
OUT = sys.argv[2] if len(sys.argv) > 2 else "docs/pipeline-charts"
os.makedirs(OUT, exist_ok=True)


def unescape_mermaid(s: str) -> str:
    # Agents sometimes HTML-escape arrows (--&gt;) ; Mermaid needs raw chars.
    return (s or "").replace("&gt;", ">").replace("&lt;", "<").replace("&amp;", "&")


def crit_color(pct):
    try:
        p = float(pct)
    except Exception:
        p = 0
    if p >= 80:
        return "#e74c3c"  # critical red
    if p >= 60:
        return "#e67e22"  # high orange
    if p >= 40:
        return "#f1c40f"  # medium amber
    return "#2ecc71"      # low green


IMP_BADGE = {"critical": "#e74c3c", "high": "#e67e22", "medium": "#f1c40f", "low": "#2ecc71"}

CSS = """
:root{--bg:#f6f8fa;--panel:#ffffff;--panel2:#f0f3f6;--line:#d0d7de;--txt:#1f2328;--muted:#57606a;--accent:#0969da;--accent2:#1a7f37}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--txt);font:15px/1.55 -apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif}
.wrap{max-width:1180px;margin:0 auto;padding:32px 22px 80px}
h1{font-size:25px;margin:0 0 4px;letter-spacing:.2px;color:#0d1117}
.sub{color:var(--muted);font-size:13px;margin:0 0 26px}
h2{font-size:19px;margin:38px 0 10px;padding-bottom:7px;border-bottom:1px solid var(--line);display:flex;align-items:center;gap:9px;color:#0d1117}
h2 .dot{width:10px;height:10px;border-radius:50%}
.ideal .dot{background:var(--accent2)} .actuel .dot{background:#bc4c00} .delta .dot{background:#cf222e}
.targets .dot{background:var(--accent)} .layers .dot{background:#8250df} .facts .dot{background:#6e7781}
.summary{background:var(--panel);border:1px solid var(--line);border-left:3px solid var(--accent);border-radius:8px;padding:13px 16px;margin:0 0 16px;color:#24292f;box-shadow:0 1px 2px rgba(31,35,40,.06)}
.diagram{background:var(--panel);border:1px solid var(--line);border-radius:10px;padding:14px;overflow:auto;margin:0 0 6px;box-shadow:0 1px 2px rgba(31,35,40,.06)}
.mermaid{display:flex;justify-content:center}
details{margin:0 0 8px;color:var(--muted);font-size:12px}
details pre{background:#f6f8fa;border:1px solid var(--line);border-radius:6px;padding:10px;overflow:auto;color:#57606a;font-size:12px}
table{width:100%;border-collapse:collapse;margin:6px 0 8px;font-size:13.5px;background:var(--panel);border:1px solid var(--line);border-radius:8px;overflow:hidden}
th,td{text-align:left;padding:9px 11px;border-bottom:1px solid var(--line);vertical-align:top}
th{color:var(--muted);font-weight:600;font-size:12px;text-transform:uppercase;letter-spacing:.4px;background:var(--panel2)}
tr:hover td{background:#f6f8fa}
.badge{display:inline-block;padding:1px 8px;border-radius:11px;font-size:11px;font-weight:700;color:#1f2328}
.critbar{position:relative;height:18px;background:#eaeef2;border-radius:9px;overflow:hidden;min-width:120px;border:1px solid var(--line)}
.critfill{position:absolute;left:0;top:0;bottom:0;border-radius:9px}
.critnum{position:relative;z-index:2;font-size:11px;font-weight:700;padding:0 8px;line-height:18px;color:#1f2328}
.ev{color:var(--muted);font-size:11.5px;font-family:ui-monospace,SFMono-Regular,Menlo,monospace}
.facts li{margin:5px 0;color:#24292f}
.facts code{background:#f0f3f6;border:1px solid var(--line);border-radius:4px;padding:1px 5px;font-size:12px;color:#0a3069}
.layer{background:var(--panel);border:1px solid var(--line);border-radius:10px;padding:14px 16px;margin:0 0 12px;box-shadow:0 1px 2px rgba(31,35,40,.06)}
.layer h3{margin:0 0 4px;font-size:16px;color:var(--accent)}
.layer .scope{color:var(--muted);font-size:12.5px;margin:0 0 8px}
.chips{display:flex;flex-wrap:wrap;gap:6px;margin:0 0 8px}
.chip{background:var(--panel2);border:1px solid var(--line);border-radius:6px;padding:2px 9px;font-size:12px;color:#24292f}
.foot{margin-top:46px;padding-top:14px;border-top:1px solid var(--line);color:var(--muted);font-size:12px}
"""


def diagram_block(label, mermaid_src):
    raw = unescape_mermaid(mermaid_src)
    return f"""
    <div class="diagram"><pre class="mermaid">{html.escape(raw)}</pre></div>
    <details><summary>source Mermaid ({label})</summary><pre>{html.escape(raw)}</pre></details>
"""


def delta_table(deltas):
    rows = []
    for d in deltas:
        c = float(d.get("criticality_pct", 0) or 0)
        col = crit_color(c)
        imp = d.get("importance", "")
        rows.append(f"""
      <tr>
        <td><strong>{html.escape(d.get('element',''))}</strong></td>
        <td>{html.escape(d.get('ideal_state',''))}</td>
        <td>{html.escape(d.get('current_state',''))}</td>
        <td><span class="badge" style="background:{IMP_BADGE.get(imp,'#888')}">{html.escape(imp)}</span></td>
        <td><div class="critbar"><div class="critfill" style="width:{c:.0f}%;background:{col}"></div><span class="critnum">{c:.0f}%</span></div></td>
        <td class="ev">{html.escape(d.get('evidence',''))}</td>
      </tr>""")
    return f"""<table><thead><tr><th>Élément</th><th>Idéal</th><th>Actuel</th><th>Importance</th><th>Criticité</th><th>Évidence</th></tr></thead><tbody>{''.join(rows)}</tbody></table>"""


def target_table(targets):
    rows = []
    for t in targets:
        rows.append(f"""
      <tr><td><strong>{html.escape(t.get('metric',''))}</strong></td>
      <td>{html.escape(str(t.get('target_value','')))}</td>
      <td>{html.escape(str(t.get('current_value','')))}</td>
      <td class="ev">{html.escape(t.get('measurable_via',''))}</td></tr>""")
    return f"""<table><thead><tr><th>Métrique</th><th>Target (idéal)</th><th>Actuel</th><th>Mesuré via</th></tr></thead><tbody>{''.join(rows)}</tbody></table>"""


def layers_block(layers):
    order = {"macro": 0, "meso": 1, "micro": 2}
    out = []
    for L in sorted(layers, key=lambda x: order.get(x.get("layer"), 9)):
        chips = "".join(f'<span class="chip">{html.escape(c)}</span>' for c in L.get("components", []))
        out.append(f"""
    <div class="layer"><h3>{html.escape(L.get('layer','').upper())} — {html.escape(L.get('scope',''))}</h3>
      <div class="chips">{chips}</div>
      <div class="scope">{html.escape(L.get('detail',''))}</div></div>""")
    return "".join(out)


def render(doc):
    parts = [f"""<!DOCTYPE html><html lang="fr"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{html.escape(doc.get('title',''))}</title>
<style>{CSS}</style></head><body><div class="wrap">
<h1>{html.escape(doc.get('title',''))}</h1>
<p class="sub">{html.escape(doc.get('doc_id',''))} · Axon · idéal vs actuel vs delta · session 71</p>"""]

    parts.append('<h2 class="ideal"><span class="dot"></span>1 · Flux idéal (tel qu\'il devrait être)</h2>')
    parts.append(f'<div class="summary">{html.escape(doc.get("ideal_summary",""))}</div>')
    parts.append(diagram_block("idéal", doc.get("ideal_mermaid", "")))

    parts.append('<h2 class="actuel"><span class="dot"></span>2 · Flux actuel (tel qu\'il est)</h2>')
    parts.append(f'<div class="summary">{html.escape(doc.get("current_summary",""))}</div>')
    parts.append(diagram_block("actuel", doc.get("current_mermaid", "")))

    if doc.get("layers"):
        parts.append('<h2 class="layers"><span class="dot"></span>Couches macro · meso · micro</h2>')
        parts.append(layers_block(doc["layers"]))

    if doc.get("deltas"):
        parts.append('<h2 class="delta"><span class="dot"></span>3 · Delta & criticité (%)</h2>')
        parts.append(delta_table(doc["deltas"]))

    if doc.get("targets"):
        parts.append('<h2 class="targets"><span class="dot"></span>Targets mesurables</h2>')
        parts.append(target_table(doc["targets"]))

    if doc.get("key_facts"):
        parts.append('<h2 class="facts"><span class="dot"></span>Faits vérifiés (MCP / code)</h2>')
        lis = "".join(f"<li>{html.escape(f)}</li>" for f in doc["key_facts"])
        parts.append(f'<ul class="facts">{lis}</ul>')

    parts.append('<div class="foot">Généré session 71 — recherche MCP-powered (3 sous-agents, project=AXO) + synthèse. Mermaid 11 (CDN). Diagrammes : si un rendu échoue, la source Mermaid est dans le &lt;details&gt; sous chaque schéma.</div>')
    parts.append("""</div>
<script type="module">
import mermaid from 'https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs';
mermaid.initialize({startOnLoad:true, theme:'default', securityLevel:'loose', flowchart:{useMaxWidth:true,htmlLabels:true}});
</script></body></html>""")
    return "".join(parts)


names = {"DOC-1-METRICS-PIPELINE": "1-pipeline-metriques.html",
         "DOC-2": "2-pipeline-ingestion.html",
         "DOC-3-ENV": "3-environnement-macro-meso-micro.html"}
written = []
for jf in sorted(glob.glob(os.path.join(SRC, "*.json"))):
    doc = json.load(open(jf))
    fname = names.get(doc.get("doc_id"), doc.get("doc_id", "doc") + ".html")
    outp = os.path.join(OUT, fname)
    open(outp, "w").write(render(doc))
    written.append(outp)
    print("wrote", outp, os.path.getsize(outp), "bytes")
print("OK", len(written), "files")
