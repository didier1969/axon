# Session 91 handoff — 2026-06-27 (grosse session)

> Audit-only. Canonique = session_pointer **CPT-AXO-052** + git log + SOLL.

## HEAD / LIVE
- `main` HEAD = **da760ce8** (+ commit opérateur `3e164376` « x »). **NON POUSSÉ** (~15 commits locaux d'avance sur origin/main 0578c223).
- **LIVE PROMU = `v0.8.0-1219-g730fd7bf`** (indexer_full, finalisé). **Wave A (aa968805 + da760ce8) committée NON PROMUE.**

## ⚡ Action suivante : promouvoir wave A
`promote_live_safe.sh --project AXO` (script durci 902104/105 → promote propre). Rend `contradiction_check` rapide (GPU) + active veto 902097. Ne pas committer pendant. Puis re-tester latence live + marquer MIL-044/045 delivered.

## Trois gros chantiers de la session

### 1. Squelette canonique structurel (DESIGN, gaté greenlight)
Dialogue de conception → **CPT-AXO-90054** + 5 Decisions (DEC-901655-659) : contrats prouvés, sceau de Merkle gaté par adéquation (anti « théâtre du sceau »), évolution gouvernée. Validé par **panel 3 experts indépendants** (VAL-AXO-148, GO-with-changes) + **prototype jetable** (VAL-149, test négatif PASS). Cœur pur S1-S5 committé (REQ-902088-095, commits 11bf9f99/4af36f78/5c1abb03). Umbrella REQ-902087. **Industrialisation (store B2 + réconciliation indexeur + surface MCP) gatée sur greenlight opérateur.**

### 2. MIL-AXO-044 — demandes clients & friction (6/7 LIVE)
Bugs + friction des 2 canaux (feedback/friction MCP) + propositions Nexus. LIVE : allocateur revision_id (902086, timestamp+nonce), coercion SUPERSEDES destructive (902098), artifact_type schema (902099), soll_manager discoverability (902082), inspect mode=source (902100), promote dev-teardown (902101). **contradiction_check (902096)** LIVE mais lent CPU. Veto (902097) committé wave A.

### 3. Nexus contradiction_check — gate anti-hallucination cross-tenant
`tasksource/ModernBERT-base-nli` exporté ONNX (`scripts/provision_nli_model.sh` → `.axon/models/nli-modernbert-base/`, 599 Mo non committé). Module `src/nli.rs` (NliClassifier, judge_global lazy). Tool `contradiction_check` (embed→shortlist ANN→re-rank NLI). **Validé end-to-end en prod** (verdict=contradicts correct) MAIS 51s/appel CPU → fix GPU EP (902103) committé+smoke-vert, pending promote. Veto opt-in sur retrieve_context_layered (902097).

## Saga promote + durcissement (MIL-AXO-045)
3 promotes contrariés : (1) post-check timeout = dev brain résiduel auto-pause indexeur → **fix 902101 step-2c teardown_dev** (validé) ; (2) step-7 échoue sur commit « x » concurrent → **fix 902105 finalize best-effort** ; (3) promote tué → runtime dégradé → **fix 902104 auto-resume** + leçon [[reference_killed_promote_degraded_runtime]]. Récup à chaque fois via `promote-live --resume --restart-live`.

## RCA notables (verify-before-fix appliqué)
- Allocateur revision : 1er fix max+1 INSUFFISANT (compteur racy) → timestamp+nonce définitif (mesuré en prod).
- Latence contradiction_check : 1er diagnostic « chargement modèle » FAUX → mesure = inférence CPU → fix GPU EP.

## Reste / blockers
Aucun bloquant. Promote wave A = action suivante. Squelette = greenlight opérateur. Friction backlog 902076 (sql guard etc.) ouvert. Commits NON poussés (à décider).
