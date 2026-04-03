#!/usr/bin/env bash
set -euo pipefail

SQL_URL="${SQL_URL:-http://127.0.0.1:44129/sql}"

post_sql() {
  local query body
  query="$(cat)"
  body="$(python3 - "$query" <<'PY'
import json
import sys

print(json.dumps({"query": sys.argv[1]}))
PY
)"
  curl -sS -X POST "$SQL_URL" -H "Content-Type: application/json" -d "$body" >/dev/null
}

post_sql <<'SQL'
UPDATE soll.Vision
SET description = 'Runtime Rust-first de verite structurelle multi-projets. Axon doit rendre le cycle de vie reel d un fichier, separer la verite graphe de la verite vectorielle, borner les rescans de sous-arbres et qualifier les runs avec des preuves durables.',
    goal = 'Donner aux developpeurs et aux agents LLM une verite deterministe, explicable et stable sur l etat reel du workspace, sans confondre detection brute, confirmation graphe et enrichissement vectoriel.',
    metadata = '{"date":"2026-04-03","source":"sota-ingestion-plan","status":"current"}'
WHERE id = 'VIS-AXO-001';
SQL

post_sql <<'SQL'
INSERT INTO soll.Pillar (id, title, description, metadata) VALUES
('PIL-AXO-007', 'Deterministic File Lifecycle', 'Chaque fichier doit exposer un etat canonique unique dans la pipeline', '{"source":"sota-ingestion-plan","status":"current"}'),
('PIL-AXO-008', 'Bounded Subtree Rescan Control', 'Les subtree hints restent des signaux de controle dedupliques et bornes', '{"source":"sota-ingestion-plan","status":"current"}'),
('PIL-AXO-009', 'Structural and Vector Truth Split', 'La completion graphe est canonique; la completion vectorielle est derivee', '{"source":"sota-ingestion-plan","status":"current"}'),
('PIL-AXO-010', 'Qualification As Runtime Proof', 'Les runs qualifies produisent les preuves de sante du systeme', '{"source":"sota-ingestion-plan","status":"current"}')
ON CONFLICT (id) DO UPDATE SET
  title = EXCLUDED.title,
  description = EXCLUDED.description,
  metadata = EXCLUDED.metadata;
SQL

post_sql <<'SQL'
INSERT INTO soll.Concept (name, explanation, rationale, metadata) VALUES
('CPT-AXO-007: Canonical File Lifecycle', 'Ajouter file_stage comme verite de cycle de vie additive par fichier', 'Eviter de surcharger File.status avec toutes les significations', '{"status":"current"}'),
('CPT-AXO-008: Subtree Hint As Bounded Control Signal', 'Un subtree hint n est pas une verite fichier; c est un signal de rescan deduplique et borne', 'Empecher les rescans de devenir une file cachee', '{"status":"current"}'),
('CPT-AXO-009: Graph Ready / Vector Ready Split', 'graph_ready et vector_ready rendent separement la verite structurelle et la verite vectorielle', 'Permettre une verite partielle honnete', '{"status":"current"}'),
('CPT-AXO-010: Qualification Run Artifact', 'Un run qualifie produit un dossier horodate avec parametres, echantillons et resume', 'Transformer la validation en preuve durable', '{"status":"current"}')
ON CONFLICT (name) DO UPDATE SET
  explanation = EXCLUDED.explanation,
  rationale = EXCLUDED.rationale,
  metadata = EXCLUDED.metadata;
SQL

post_sql <<'SQL'
INSERT INTO soll.Milestone (id, title, status, metadata) VALUES
('MIL-AXO-006', 'Canonical File Lifecycle Rollout', 'in_progress', '{"date":"2026-04-03","evidence":"file_stage migration"}'),
('MIL-AXO-007', 'Bounded Subtree Hint Telemetry', 'in_progress', '{"date":"2026-04-03","evidence":"runtime observability"}'),
('MIL-AXO-008', 'Qualification Harness As Standard Gate', 'in_progress', '{"date":"2026-04-03","evidence":"qualify_ingestion_run.py"}')
ON CONFLICT (id) DO UPDATE SET
  title = EXCLUDED.title,
  status = EXCLUDED.status,
  metadata = EXCLUDED.metadata;
SQL

post_sql <<'SQL'
INSERT INTO soll.Requirement (id, title, description, status, priority, metadata) VALUES
('REQ-AXO-008', 'One Canonical Current Stage Per File', 'Chaque fichier canonique dans File doit exposer un file_stage unique et coherent avec son etat courant.', 'current', 'P1', '{"source":"sota-ingestion-plan"}'),
('REQ-AXO-009', 'Bounded Subtree Rescans', 'Les rescans issus d evenements de repertoire doivent rester dedupliques, bornes et observables, sans devenir une file implicite concurrente.', 'current', 'P1', '{"source":"sota-ingestion-plan"}'),
('REQ-AXO-010', 'Structural Truth Queryable Independently Of Vector Truth', 'La disponibilite graphe doit etre observable separement de la disponibilite vectorielle via des indicateurs explicites.', 'current', 'P1', '{"source":"sota-ingestion-plan"}'),
('REQ-AXO-011', 'Dashboard And SQL Must Share The Same Lifecycle Vocabulary', 'Les surfaces cockpit, MCP et SQL doivent raconter la meme histoire de cycle de vie et de readiness.', 'current', 'P1', '{"source":"sota-ingestion-plan"}'),
('REQ-AXO-012', 'Qualification Runs Must Produce Durable Evidence', 'Les runs de qualification doivent produire des artefacts horodates avec parametres, echantillons, resume et logs.', 'current', 'P1', '{"source":"sota-ingestion-plan"}')
ON CONFLICT (id) DO UPDATE SET
  title = EXCLUDED.title,
  description = EXCLUDED.description,
  status = EXCLUDED.status,
  priority = EXCLUDED.priority,
  metadata = EXCLUDED.metadata;
SQL

post_sql <<'SQL'
INSERT INTO soll.Decision (id, title, description, context, rationale, status, metadata) VALUES
('DEC-AXO-008', 'Keep File.status For Compatibility While Adding file_stage', 'file_stage est ajoute comme verite additive sans casser immediatement File.status.', 'Les outils existants utilisent encore File.status.', 'Migrer sans briser le runtime existant.', 'accepted', '{"source":"sota-ingestion-plan"}'),
('DEC-AXO-009', 'Graph Ready And Vector Ready Are Distinct Signals', 'graph_ready et vector_ready restent deux signaux separes.', 'La completion structurelle et la completion vectorielle n ont pas la meme semantique.', 'Permettre une verite partielle honnete.', 'accepted', '{"source":"sota-ingestion-plan"}'),
('DEC-AXO-010', 'Qualification Script Is The Standard Runtime Evidence Surface', 'scripts/qualify_ingestion_run.py devient l entree standard pour qualifier un run.', 'Les diagnostics ad hoc ne suffisent plus.', 'Produire des preuves reproductibles au lieu d impressions de console.', 'accepted', '{"source":"sota-ingestion-plan"}'),
('DEC-AXO-011', 'Subtree Hints Stay Control Signals, Never File Truth', 'Un subtree hint reste une commande de rescan bornee, jamais un substitut a l etat canonique d un fichier.', 'Les directory events peuvent affamer la promotion utile.', 'Clarifier la frontiere entre bruit de detection et verite canonique.', 'accepted', '{"source":"sota-ingestion-plan"}')
ON CONFLICT (id) DO UPDATE SET
  title = EXCLUDED.title,
  description = EXCLUDED.description,
  context = EXCLUDED.context,
  rationale = EXCLUDED.rationale,
  status = EXCLUDED.status,
  metadata = EXCLUDED.metadata;
SQL

post_sql <<'SQL'
INSERT INTO soll.Validation (id, method, result, timestamp, metadata) VALUES
('VAL-AXO-004', 'qualification-no-oom', 'planned', 1775208900, '{"scope":"5m full run","status":"planned"}'),
('VAL-AXO-005', 'qualification-subtree-bounded', 'planned', 1775208900, '{"scope":"subtree hint churn","status":"planned"}'),
('VAL-AXO-006', 'qualification-dashboard-sql-coherence', 'planned', 1775208900, '{"scope":"same sampling window","status":"planned"}')
ON CONFLICT (id) DO UPDATE SET
  method = EXCLUDED.method,
  result = EXCLUDED.result,
  timestamp = EXCLUDED.timestamp,
  metadata = EXCLUDED.metadata;
SQL

post_sql <<'SQL'
INSERT INTO soll.EPITOMIZES (source_id, target_id) VALUES
('PIL-AXO-007', 'VIS-AXO-001'),
('PIL-AXO-008', 'VIS-AXO-001'),
('PIL-AXO-009', 'VIS-AXO-001'),
('PIL-AXO-010', 'VIS-AXO-001');
SQL

post_sql <<'SQL'
INSERT INTO soll.EXPLAINS (source_id, target_id) VALUES
('CPT-AXO-007: Canonical File Lifecycle', 'REQ-AXO-008'),
('CPT-AXO-008: Subtree Hint As Bounded Control Signal', 'REQ-AXO-009'),
('CPT-AXO-009: Graph Ready / Vector Ready Split', 'REQ-AXO-010'),
('CPT-AXO-010: Qualification Run Artifact', 'REQ-AXO-012');
SQL

post_sql <<'SQL'
INSERT INTO soll.BELONGS_TO (source_id, target_id) VALUES
('REQ-AXO-008', 'PIL-AXO-007'),
('REQ-AXO-009', 'PIL-AXO-008'),
('REQ-AXO-010', 'PIL-AXO-009'),
('REQ-AXO-011', 'PIL-AXO-010'),
('REQ-AXO-012', 'PIL-AXO-010');
SQL

post_sql <<'SQL'
INSERT INTO soll.SOLVES (source_id, target_id) VALUES
('DEC-AXO-008', 'REQ-AXO-008'),
('DEC-AXO-009', 'REQ-AXO-010'),
('DEC-AXO-010', 'REQ-AXO-012'),
('DEC-AXO-011', 'REQ-AXO-009');
SQL

post_sql <<'SQL'
INSERT INTO soll.TARGETS (source_id, target_id) VALUES
('MIL-AXO-006', 'REQ-AXO-008'),
('MIL-AXO-007', 'REQ-AXO-009'),
('MIL-AXO-008', 'REQ-AXO-012');
SQL

post_sql <<'SQL'
INSERT INTO soll.VERIFIES (source_id, target_id) VALUES
('VAL-AXO-004', 'REQ-AXO-008'),
('VAL-AXO-005', 'REQ-AXO-009'),
('VAL-AXO-006', 'REQ-AXO-011');
SQL
