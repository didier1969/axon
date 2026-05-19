# Prompt-specifier à contexte isolé — méthodologie LLM-discipline

**Status** : proposition de méthodologie, non encore adoptée. Issue d'une conversation operator/Claude du 2026-05-14 sur les biais récurrents des LLMs sur Axon (bloat, code mort, sur-ingénierie, micro-management). Candidate à devenir `PIL-AXO-010` + une famille `REQ-AXO-{...}` d'implémentation.

**Auteurs originaires** : Didier (intuition initiale "dialogue plutôt que rappel de masse" → "ceremony gates" → "prompt généré par sub-agent à contexte minimal"). Claude (formalisation).

---

## 1. Problème

Les LLMs qui produisent du code sur Axon (et tout repo non-trivial) manifestent un ensemble stable de pathologies :

- **Bloat** : helpers de 30 lignes là où 5 inline suffisent ; abstractions premature ; wrappers "for clarity".
- **Code mort résiduel** : refactor ajoute le nouveau chemin et laisse l'ancien avec un TODO jamais réalisé.
- **Réimplémentation** : fonction écrite alors qu'un symbole quasi-identique existe déjà (visible via `query`/`semantic_clones` non invoqués).
- **Scope creep** : un fix de bug livre 200 lignes au lieu de 10, dont 80% non liées au bug.
- **Symptom-fix** : un guard / un flag / une option au lieu d'une correction racine.
- **Doc accumulation** : working-notes créées au lieu d'updater le CPT canonique.

Le SOLL contient déjà les guidelines anti-bloat (GUI-PRO-013/014/015/017/018/020/030). Le LLM les lit à l'init et les oublie à l'édition. **Le gap n'est pas dans l'ontologie ; il est dans le moment d'exécution.**

Mécanismes envisagés et leurs limites :

| Approche | Limite |
|---|---|
| Rappel ambient des guidelines dans chaque tool response | Survol cognitif ; pas d'articulation forcée |
| Dialogue socratique par-edit (5-10 questions) | Centaines de variantes, scale catastrophique, interrompt le flow |
| 3 gates cérémoniels (branche / commit / promote) sur règles abstraites | Goodhart's Law : LLM optimise l'évasion des détecteurs, pas le comportement |
| Détecteurs structurels seuls (`anomalies`, `semantic_clones`) | Détectent symptôme post-écriture ; n'évitent pas la production |

## 2. Insight central

**Séparer l'auteur du plan de l'exécutant du plan, par isolation de contexte.**

C'est isomorphe à la séparation product manager / engineer dans une équipe humaine. Celui qui spécifie le travail ne doit pas être celui qui l'implémente, parce que l'exécutant rationalise toujours vers ce qu'il sait déjà faire et vers les confort-paths accumulés dans son contexte.

Pour les LLMs, l'isolation de contexte est l'outil exact qui casse la boucle de self-bullshit : un sub-agent sans accès à la conversation main, sans accès aux décisions de session précédentes, sans accès aux working-notes accumulées, est **structurellement incapable** de rationaliser le bloat du main LLM. Il produit un prompt minimal parce qu'il n'a pas le matériau pour rationaliser autre chose.

Le prompt qu'il produit devient alors un **contrat falsifiable** que le main LLM doit honorer. Le commit-gate vérifie l'alignement diff-vs-prompt en sus de tout détecteur structurel.

## 3. Mécanisme — 5 étapes

1. **Réception du REQ.** Main LLM reçoit "exécute `REQ-AXO-X`". `Write`/`Edit` sont **bloqués** jusqu'à étape 4.
2. **Invocation sub-agent.** Main LLM appelle `axon_prompt_specifier` avec uniquement le REQ ID. Le sub-agent boot avec contexte minimal pré-défini (voir §5).
3. **Génération prompt.** Sub-agent produit un prompt structuré au format §4. Persisté en SOLL comme `VAL-AXO-{N}` rattaché au REQ via une edge `PRESCRIBES`.
4. **Déblocage Write/Edit.** Main LLM reçoit le prompt + l'autorisation d'éditer. La main LLM peut consulter le prompt à tout moment.
5. **Commit-gate avec alignment check.** `axon_pre_flight_check` (forcé, plus voluntary) vérifie : diff ⊆ scope_in, diff ⊄ scope_out, forbidden_patterns absents, diff_budget respecté, test_criterion vert. Failure → blocked avec evidence ; LLM fix ou justify (justify devient nouvelle revision du prompt).

## 4. Template du prompt

Le sub-agent produit un objet JSON structuré, pas de prose. Format normatif :

```json
{
  "req_id": "REQ-AXO-345",
  "goal": "one-sentence testable success criterion",
  "scope_in": {
    "files": ["src/axon-core/src/...rs", ...],
    "symbols": ["fully::qualified::path", ...],
    "behaviors": ["FIFO ordering of buffered ingress", ...]
  },
  "scope_out": {
    "new_files_allowed": 0,
    "new_pub_functions_allowed": 0,
    "new_env_vars_allowed": 0,
    "new_dependencies_allowed": 0,
    "new_doc_files_allowed": 0
  },
  "existing_code_to_modify": [
    {"symbol": "ingress_buffer::compare_buffered", "rationale": "FIFO tiebreak via seq field"},
    {"symbol": "upsert_graph_v2_batch", "rationale": "add file-table UPSERT"}
  ],
  "forbidden_patterns": [
    "new helper function without 2+ callers",
    "// TODO comments left in diff",
    "log statements above DEBUG level",
    "config flag toggling new behavior"
  ],
  "test_criterion": {
    "type": "cargo_test",
    "target": "axon_core::ingress_buffer::tests::fifo_tiebreak_respects_seq"
  },
  "diff_budget": {
    "max_lines_added": 150,
    "max_files_created": 0,
    "max_files_modified": 3
  },
  "dependencies": ["DEC-AXO-085 (REQ ID format)", "PIL-AXO-007 (graph pipeline)"],
  "rollback": "git reset --hard <pre-branch SHA>; no .axon/ artefacts to clean"
}
```

Chaque champ est **falsifiable mécaniquement** :
- `scope_in.files` ⊇ files in `git diff --name-only` → checkable
- `scope_out.new_files_allowed` ≥ `git diff --name-only --diff-filter=A | wc -l` → checkable
- `forbidden_patterns` → grep diff
- `test_criterion` → cargo test resolves
- `diff_budget.max_lines_added` ≥ `git diff --shortstat | awk ...`

## 5. Contexte autorisé / interdit pour le sub-agent

**Allowed** :
- REQ body + acceptance criteria (single `soll_query_context` call)
- Guidelines filtrées par `req.type` :
  - `fix` → GUI-PRO-015, -017, -030
  - `feature` → GUI-PRO-013, -018, -023
  - `refactor` → GUI-PRO-017, -018, -021
  - `perf` → GUI-PRO-009, -030
- MCP tools pour exploration ciblée : `query`, `retrieve_context`, `impact`, `why`, `path`, `semantic_clones`, `anomalies`
- Pillars projet (pour comprendre les invariants architecturaux)
- 10 derniers REQs livrés sur le même projet, avec leur fate (`delivered` / `superseded` / `reverted`) — signal d'humilité

**Forbidden** :
- Historique de la conversation main
- Working-notes de la session courante
- Edits précédents de la session
- REQs adjacents in-flight (évite scope creep)
- Full file contents (force le sub-agent à `query` incrémentalement plutôt que charger)

L'isolation est mécaniquement appliquée par le gate `axon_prompt_specifier` — il ne reçoit pas le conversation_id du main.

## 6. Alignment check au commit-gate

Deux étages :

**Étage déterministe (toujours)** :
- File set : `git diff --name-only` ⊆ `scope_in.files` ∪ new files dans budget
- New files count ≤ `scope_out.new_files_allowed`
- New pub fn count : AST scan diff, ≤ `scope_out.new_pub_functions_allowed`
- Forbidden patterns : grep diff against chaque pattern
- Lines added ≤ `diff_budget.max_lines_added`
- `cargo test <test_criterion.target>` returns 0
- Détecteurs Axon (`anomalies`, `semantic_clones`, `architectural_drift`) sur diff

**Étage sémantique (optionnel, sub-agent fresh)** :
- Un second sub-agent à contexte minimal reçoit `goal` + `diff` + lit le résultat des détecteurs. Question unique : "ce diff accomplit-il le goal sans rien ajouter d'inutile ?" Réponse booléenne + justification.

Si étage déterministe fail → blocked, evidence retournée. Main LLM doit : (a) fix le diff, ou (b) demander une révision du prompt au sub-agent (qui voit le nouveau contexte minimal seulement — pas la conversation autour de la failure).

## 7. Intégration dans Axon

**Outils MCP à ajouter** :
- `axon_prompt_specifier(req_id) -> Prompt` — invoque sub-agent à contexte isolé.
- `axon_prompt_validate(req_id, diff) -> AlignmentReport` — alignment check étage déterministe.

**Outils existants à modifier** :
- `axon_init_project.kickoff_bundle` : ajout d'un champ `prompt_required: true` indiquant que toute exécution de REQ passe par le specifier.
- `axon_pre_flight_check` : devient forcé (non-voluntary) ; consomme `axon_prompt_validate`.
- `axon_commit_work` : refuse de stage si `axon_prompt_validate` n'est pas vert.

**SOLL nodes à créer** :
- `CPT-AXO-prompt-template` : template canonique versionné. Évolution manuelle par operator.
- `PIL-AXO-010` : "Discipline d'exécution LLM via prompt-specifier isolé" — fusionne avec les détecteurs structurels comme implémentation.

**Edges nouvelles** :
- `(REQ) -PRESCRIBES-> (VAL)` où VAL est le prompt persisté.
- `(VAL) -VERIFIED_BY-> (Commit SHA)` pour audit a posteriori.

## 8. Failure modes & mitigations

| Failure mode | Cause | Mitigation |
|---|---|---|
| Context-starvation du sub-agent | Fait critique non-évident hors REQ body | Sub-agent a accès `query`/`retrieve_context` MCP — peut creuser à la demande |
| Récursivité du template | Qui spécifie le specifier ? | Template versionné en SOLL `CPT-AXO-prompt-template`, évolution manuelle uniquement |
| Coût token | ~5-20k tokens par REQ | Acceptable car par-REQ, pas par-edit ; négligeable face au gain qualité |
| Sub-agent paresseux | Produit prompt trivial qui passe tout | Validation : prompt doit contenir ≥3 forbidden_patterns spécifiques et un test_criterion résoluble |
| Main LLM ignore le prompt | Édite hors scope, blame le specifier | Commit-gate forcé bloque — pas de bypass possible sans operator override explicite |
| Scope change légitime mid-flight | Le REQ s'avère plus complexe | Main LLM rappelle `axon_prompt_specifier` avec annotation "revision N+1" ; SOLL conserve les deux versions du prompt — audit visible |
| Prompt template évolue sans contrôle | Drift de méthodologie | Template versionné ; review operator obligatoire pour update |

## 9. Exemple travaillé — REQ-AXO-345 rétroactif

Le fix cascade pipeline v2 (FIFO drain + file-table population) shippé en session 32. Diff réel : `+187 / -23 lignes` sur 4 fichiers. Tests ajoutés : 2.

**Ce que le prompt aurait spécifié :**

```json
{
  "req_id": "REQ-AXO-345",
  "goal": "Ingress drain respects FIFO across priority ties + populate public.file on every graph upsert so FileIngressGuard hydrates correctly on boot",
  "scope_in": {
    "files": [
      "src/axon-core/src/indexer/ingress_buffer.rs",
      "src/axon-core/src/indexer/graph_ingestion.rs"
    ],
    "symbols": [
      "ingress_buffer::IngressBuffer",
      "ingress_buffer::compare_buffered",
      "graph_ingestion::upsert_graph_v2_batch"
    ],
    "behaviors": [
      "FIFO tiebreak on (priority, seq) instead of (priority, path)",
      "UPSERT public.file row per batch member"
    ]
  },
  "scope_out": {
    "new_files_allowed": 0,
    "new_pub_functions_allowed": 0,
    "new_env_vars_allowed": 0,
    "new_dependencies_allowed": 0,
    "new_doc_files_allowed": 0
  },
  "existing_code_to_modify": [
    {"symbol": "BufferedIngress enum", "rationale": "add seq: u64 to File/Tombstone variants"},
    {"symbol": "compare_buffered", "rationale": "tiebreak by seq ASC"},
    {"symbol": "upsert_graph_v2_batch", "rationale": "INSERT ... ON CONFLICT for public.file"}
  ],
  "forbidden_patterns": [
    "new helper function for seq generation (use AtomicU64 inline)",
    "config flag to disable FIFO (regression risk)",
    "doc comment longer than 1 line on modified functions",
    "INFO log on hot path (use DEBUG)"
  ],
  "test_criterion": {
    "type": "cargo_test",
    "target": "axon_core::indexer::ingress_buffer::tests::fifo_tiebreak_respects_seq"
  },
  "diff_budget": {
    "max_lines_added": 200,
    "max_files_created": 0,
    "max_files_modified": 2
  },
  "dependencies": ["PIL-AXO-007 (graph-first pipeline)", "REQ-AXO-289 (streaming v2)"],
  "rollback": "git reset --hard <pre-branch>; rebuild candidate; promote previous manifest"
}
```

**Diff réel vs prompt** :
- `max_files_modified: 2` vs reality 4 → **alignment failure**. Le specifier aurait sous-estimé. Main LLM aurait dû soit demander revision (legit — le bug touchait plus de surface que prévu), soit splitter le commit.
- Cette failure est **un signal utile**, pas un blocage stupide : elle force la conversation operator-LLM sur le périmètre réel du bug.

## 10. Plan d'implémentation

Ordre :

1. **`CPT-AXO-prompt-template`** dans SOLL — fixer le format JSON canonique. ~1 jour.
2. **`axon_prompt_specifier` MCP tool** — sub-agent isolé contexte, génère JSON template. ~3 jours.
3. **`axon_prompt_validate` MCP tool** — alignment check étage déterministe sur diff git. ~2 jours.
4. **`axon_pre_flight_check` forced** — refuse commit sans validate vert. ~1 jour.
5. **Pilote sur 3 REQs réels** — mesurer diff size, supersede rate, satisfaction operator. ~1 semaine de runtime.
6. **`axon_init_project` kickoff_bundle update** — pousser `prompt_required: true` au LLM. ~0.5 jour.
7. **Étage sémantique optionnel** — second sub-agent fresh sur diff. ~3 jours. À retarder si étage déterministe suffit.

## 11. Métriques de succès falsifiables

Comparer sur deux fenêtres de 4 semaines (avant / après instauration) :

| Métrique | Source | Hypothèse |
|---|---|---|
| Lignes diff par REQ délivré | `git log --shortstat` + SOLL `delivered` | -30% médiane |
| Supersede-rate à 7 jours | SOLL Revision `supersedes_at - delivered_at` | -50% |
| Ratio edit/create dans les commits | `git log --diff-filter=A vs M` | +40% (plus d'edits, moins de creates) |
| Nombre de fichiers `working-notes` par session | filesystem diff | -60% |
| Détecteurs `anomalies` orphans post-commit | `mcp_axon_anomalies` sur HEAD | -70% |
| Operator interventions correctives par REQ | SOLL Originator log | -50% |

Si ces métriques ne bougent pas significativement après 4 semaines d'usage sur les REQs nouveaux, la méthodologie a échoué et doit être révisée. C'est l'engagement falsifiable.

---

## Tags

`prompt-specifier`, `context-isolation`, `llm-discipline`, `methodology-proposal`, `pil-axo-010-candidate`, `goodhart-resistant`, `falsifiable`, `session-2026-05-14`
