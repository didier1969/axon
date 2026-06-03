# Session 71 — W1 (REQ-91560) résidu : RCA de la queue dure

État fin session 71 : **172 passed / 50 failed** (était 131/91 au début). 2 commits root-cause
(`fef51f65`, `ee17f881`). Le résidu 50 = queue per-test, décomposée ci-dessous avec le chemin de fix.

## Cause racine #1 (la plus grosse) — `ist_fixtures.rs` hors isolation + table legacy CALLS

`src/axon-core/src/test_support/ist_fixtures.rs::create_test_server_with_ist_seed` (l.347-362) :
- utilise `GraphStore::new(&db_root)` (l.354) → résout l'URL PG depuis `AXON_DEV_DATABASE_URL`
  = **DB partagée `axon_dev`**, PAS le clone template avec mes triggers auto-seed. C'est le chemin
  pré-isolation que REQ-915 devait retirer mais qui survit pour ce helper.
- `CallFixture::insert_sql` (l.164-171) insère dans la table legacy **`CALLS`** (retirée AGE-era,
  n'existe pas) → Writer Error → `.unwrap()` casse.

**Preuve empirique** : sur un clone frais du template, `INSERT INTO Symbol ... 'PRJ'` et
`INSERT INTO GraphProjectionState ...` réussissent (`INSERT 0 1`). Donc les échecs Symbol/GPS en test
sont du **poison writer-pool** : un insert antérieur (CALLS legacy, ou FK sur DB partagée sans triggers)
échoue et met la connexion poolée en état « transaction aborted », faisant échouer tout le reste du test.

Tests touchés (5 via `create_test_server_with_ist_seed` / `IstSeed`) : test_axon_inspect + audit/path/why
qui utilisent le builder IstSeed.

**Fix (correct, DRY)** : déplacer l'infra `TestDb` + `ensure_template_once` + `sweep` + triggers de
`mcp/tests/mod.rs` (cfg-test) vers `test_support/` (toujours compilé), puis :
1. `create_test_server_with_ist_seed` clone le template via `GraphStore::new_with_database(&db_root, &clone_url)`.
2. Rewrite `CALLS`→`ist.Edge (..., relation_type='CALLS', ..., created_at_ms=0)` dans `CallFixture::insert_sql`.
3. Idem si CONTAINS/IMPLEMENTS apparaissent. soll.Node fixture (l.221) hérite des triggers SOLL.

## Cause racine #2 — GraphProjectionState omet project_code

6 inserts GPS (context_and_analysis) omettent `project_code` (NOT NULL FK, sans défaut). anchor_id =
`prj::...`. Fix : trigger test-template `BEFORE INSERT ON ist.GraphProjectionState` dérivant
`project_code := upper(split_part(anchor_id,'::',1))` + seed ist.Project. (À ajouter dans
`apply_test_autoseed_triggers`.) Note : ces tests passent par le chemin mod.rs (template), donc le trigger
suffit ; mais ceux via ist_fixtures restent bloqués par la cause #1.

## Cause racine #3 — ids SOLL non-canoniques dans les fixtures

Ex. `INSERT INTO soll.Node ... 'REQ-AXO-212a'` → viole le CHECK `id ~ '^[A-Z]{3}-[A-Z][A-Z0-9]{2}-[0-9]{3,}$'`
(suffixe lettre 'a'). Per-test : remplacer par un id canonique (digits-only). Idem 'GUI-REQ145-001'.
NON trigger-able (l'id est le choix du test). ~7 tests soll.Node.

## Cause racine #4 — dérive de contrat (catégorie D, per-test)

- `document_intent` / `soll_apply_plan` / `soll_manager create` exigent `attach_to` (MIL-AXO-020) ; les
  tests appellent sans → erreur. Fix : passer `attach_to` canonique. Note : `document_intent` ne trouve
  pas de Pillar AXO à inférer car le template seed n'a pas de Pillar AXO (catégorie A résiduelle) — soit
  seeder un PIL-AXO minimal dans le template, soit les tests fournissent attach_to.
- `test_axon_soll_manager_can_create_and_update_vision` : Vision création interdite via soll_manager
  (MIL-AXO-020) → le test doit passer par axon_init_project.
- Wording : `content.contains("SOLL entity created")` → vérifier le message actuel.
- SUPERSEDES same-type, status vocab canonical (DEC-PRO-100).

## Cause racine #5 — assertions audit/anomaly/impact (context_and_analysis)

test_axon_health_god_objects, test_anomalies_*, test_axon_architectural_drift, test_vcr2_impact :
atteignent l'assertion (FK ok) mais le tool ne trouve pas le finding attendu. À vérifier : les edges
réécrits (`ist.Edge` relation 'CONTAINS'/'CALLS') sont-ils lus par le tool avec le bon relation_type ?
god_objects compte peut-être par fichier/LOC nécessitant IndexedFile.size — fixture à compléter.

## Ordre d'attaque recommandé (session 72, contexte frais)

1. **Cause #1** (refactor isolation ist_fixtures) → débloque 5 tests + prévient la divergence future. PLUS GROS ROI.
2. **Cause #2** (trigger GPS) → 3-6 tests, trivial.
3. **Cause #3** (ids canoniques) → ~7 tests, mécanique.
4. **Cause #4** (attach_to + Pillar AXO template seed) → ~10 tests.
5. **Cause #5** (assertions audit) → vérifier relation_type lu par les tools.

Probable convergence 158/158 en 1 session fraîche avec ce plan.
