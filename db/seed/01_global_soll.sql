-- DEC-AXO-082 seed half — canonical SOLL seed applied via `psql -f` on every
-- runtime startup (after db/ddl/*.sql DDL files). Each statement is idempotent
-- (`ON CONFLICT DO NOTHING`) so re-running on a warm DB is a few-ms no-op.
--
-- Scope (this slice, REQ-AXO-91577) : restore `PRO` sentinel project_code
-- into `soll.ProjectCodeRegistry`. The PRO namespace holds Axon-produit's
-- cross-tenant methodology surface (GUI-PRO-*, SKI-PRO-*, PRT-PRO-* per
-- Pillar PIL-AXO-9003 Two-Sided Identity). 32 grandfathered GUI-PRO-* nodes
-- already exist with project_code='PRO' but the registry row was lost
-- post-bootstrap, breaking the canonical mutation API for the cross-tenant
-- surface.
--
-- The matching SOLL Registry row is created by the Rust path
-- (`ensure_soll_registry_row`) on first mutation reference. Future slices
-- of DEC-AXO-082 (separate REQ) will migrate the 20+ GUI-PRO-* guidelines
-- currently hardcoded in `graph_bootstrap::seed_global_guidelines()` into
-- this file as well, retiring the Rust seed path entirely.

INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name, session_pointer_json)
VALUES ('PRO', '(sentinel:cross-project-methodology)', 'System Global Namespace', NULL)
ON CONFLICT (project_code) DO NOTHING;
