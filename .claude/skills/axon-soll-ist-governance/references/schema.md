# Axon DuckDB Schema: SOLL & IST Layers (current)

## SOLL core
- `soll.Vision(id, title, description, goal, metadata)`
- `soll.Pillar(id, title, description, metadata)`
- `soll.Requirement(id, title, description, status, priority, metadata, owner, acceptance_criteria, evidence_refs, updated_at)`
- `soll.Decision(id, title, description, context, rationale, status, metadata, supersedes_decision_id, impact_scope, updated_at)`
- `soll.Milestone(id, title, status, metadata)`
- `soll.Validation(id, method, result, timestamp, metadata)`
- `soll.Concept(name, explanation, rationale, metadata)`
- `soll.Stakeholder(name, role, metadata)`
- `soll.Registry(project_slug, id, last_pil, last_req, last_cpt, last_dec, last_mil, last_val)`

## SOLL v2 governance tables
- `soll.Revision(revision_id, author, source, summary, status, created_at, committed_at)`
- `soll.RevisionChange(revision_id, entity_type, entity_id, action, before_json, after_json, created_at)`
- `soll.RevisionPreview(preview_id, author, project_slug, payload, created_at)`
- `soll.Traceability(id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at)`

## IST and bridge relations
- IST core: `File`, `Symbol`, `Chunk`, `ChunkEmbedding`, `GraphProjection`, `GraphEmbedding`, `Project`
- Bridge relations used by SOLL workflows:
- `SUBSTANTIATES(source_id, target_id)`
- `IMPACTS(source_id, target_id)`
- SOLL relations:
- `soll.BELONGS_TO`, `soll.EXPLAINS`, `soll.SOLVES`, `soll.TARGETS`, `soll.VERIFIES`, `soll.ORIGINATES`, `soll.SUPERSEDES`, `soll.EPITOMIZES`, `soll.CONTRIBUTES_TO`, `soll.REFINES`
