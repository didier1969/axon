# Session 71 — Évaluation MIL-AXO-027 (garantie opérateur)

Audit-only narrative. Canonical = VAL node attaché à MIL-AXO-027 + ce fichier comme evidence.
Méthode : workflow multi-agent adversarial (14 agents, 5/8 slices évaluées + réfutation + synthèse ;
S0/S2/S8 eval-agents ont échoué StructuredOutput, S2 reconstruit par le synthétiseur) PLUS vérification
SQL indépendante par l'agent principal (exigence opérateur « ta propre expérience »).

## Verdict global

**ENGAGER MIL-27 ÉTROITEMENT ET EMPIRIQUEMENT — pas comme un bloc.** Aucune slice ne franchit 70 % de
confiance en tant que BUILD après réfutation. Trois prémisses expert porteuses sont empiriquement cassées
dans l'environnement réel de l'opérateur (Claude Code) :

1. **Cold-start tax 15-50K (200-800 tokens × 67 tools)** — INVALIDÉE. Claude Code defer les ~80 tools via
   ToolSearch (≤5 schémas surfacés par intent). La surface de routing effective est déjà ≤5, sous la
   cible ≤15. La consolidation (Slice 5) n'achète quasi rien dans le client réellement utilisé.
2. **Course lost-update défendue par MVCC (Slice 4)** — JAMAIS observée. 0 paire d'updates même-node
   rapprochés dans tout l'historique RevisionChange (vérif SQL indépendante). Le système est
   single-writer-per-node en pratique. Le seul incident concurrent du corpus (REQ-AXO-328) est une
   collision fichier-source déjà couverte par GUI-PRO-104 (agent-locks), couche que Slice 4 ne touche pas.
3. **Compression cold-start ±20% → 6-8K (Slice 6)** — calibrée sur un baseline fictif 15-20K. Corpus
   pilote réel (8 nodes) = 20 651 chars ≈ 5,2K tokens. La bande cible (6-8K « après ») est AU-DESSUS du
   baseline réel ; le 2,5× headline est irréalisable sur ce corpus.

## Ce qui SURVIT (valeur narrow réelle)

- **Body-read tax réel & indépendant des deferred-tools** : GUI-PRO-102 Phase A lit chaque Pillar+Guideline
  en entier au cold-start. Vérif SQL : 10 PIL current + 8 GUI AXO current = 48 607 chars ≈ 12K tokens.
  336 nodes > 2K chars (max 8152 = PIL-AXO-9003). Compresser la prose gonflée a une valeur locale 3-5×
  réelle — mais c'est un gain narrow, indépendant de la machinerie contrat v4.
- **Slice 6 pilote comme gate empirique** (la meilleure discipline du milestone — reframe en gate-to-NOT-build).
- **Slice 8 self-introspection** : adresse un gap vécu (le LLM doit bash+devenv+psql pour comparer dev/live
  IST — fait en Phase A cette session). Additif, read-only, indépendant de la chaîne v4/consolidation.
- **Slice 3-C6** : markdown comme MODE param (pas tool séparé) — ergonomie triviale, découplée du reste.

## Tableau %valeur par élément (post-réfutation)

| Slice | REQ | Valeur | Conf. | Blast | Recommandation |
|---|---|---|---|---|---|
| S0 mapping 67→15 | 901783 | ~10% | n/a | low | moot (S5 différée) ; doc-only, non chiffré (agent échoué) |
| S2 Layer A metadata | 901785 | 58% | 55% | medium | substrat ; build APRÈS dry-run pilote S6 valide le data-model |
| S3 Layer B envelope | 901786 | 26% | 30% | high | extraire C6 markdown seul ; différer envelope/render_hash (4/9 champs bloqués sur Layer A greenfield) |
| S4 MVCC | 901787 | 13% | 20% | high | **DÉFÉRER** indéfiniment (0 collision observée) |
| S5 consolidation 67→15 | 901789 | 18% | 24% | very_high | **DÉFÉRER** (prémisse invalidée client deferred ; blast max sur contrat MCP vivant) |
| S6 pilote ±20% | 901790 | 48% | 74%* | low | **EXPÉRIENCE D'ABORD** (zéro-code) ; *74% en tant qu'expérience, pas build |
| S7 migration 588 nodes | 901791 | 22% | 12% | high | **DÉFÉRER** (fusion non-destructive sur base never-delete, 0 consommateur v4) |
| S8 self-introspection | 901793 | ~50%** | n/a | low-med | **candidat build standalone** (gap opérateur vécu) ; non chiffré rigoureusement (agent échoué) |

\* S6 : confiance 74 % uniquement en tant qu'expérience (« l'expérience EST le livrable »), pas en tant que build.
\** S8 : estimation agent principal ; chevauchement partiel avec status/project_status/diagnose_indexing,
valeur nette = vue cross-instance dev+live en 1 call + recent_mutations + surface scripts.

## Vérifications SQL indépendantes (agent principal, pas sous-agents)

| Claim | SQL | Résultat |
|---|---|---|
| 0 collision même-node | `RevisionChange` gaps consécutifs par entity_id | 0 paire |
| Greenfield v4 | `metadata ? 'schema_version'` / `'content_hash'` sur 1735 nodes | 0 / 0 |
| metadata legacy déjà peuplé | `metadata ? 'priority'` / `'rationale'` | 678 / 142 |
| Corpus pilote réel | sum(length(description)) des 8 nodes pilote | 20 651 chars ≈ 5,2K tok |
| Body-read tax Phase A | 10 PIL + 8 GUI current | 48 607 chars ≈ 12K tok |
| Nodes prose / bloated | type∈prose / length>2000 | 588 / 336 |

## Prochaine étape recommandée (cheap, code-free, retire le risque avant tout build)

1. **Expérience S6 zéro-code** : tokeniser le baseline réel + hand-encoder 2 nodes extrêmes
   (PIL-AXO-9003 8152c worst-case, GUI-PRO-013 192c compact) en short+rules ; tester si un LLM frais
   reconstruit l'intent depuis short+rules SEULS. Build Layer A (S2) seulement si compression médiane ≥2×
   ET récupération d'intent tient.
2. **A/B routing S5** code-free (eager vs deferred client) pour DOCUMENTER que le harness gagne déjà.
3. **Décision opérateur** : S8 (build standalone ?) + S3-C6 (ergonomie triviale) + compression ciblée des
   ~336 nodes prose gonflés (valeur narrow indépendante de v4).

Evidence brute : `2026-06-03-session-71-mil27-evaluation-raw.json` (workflow complet, 14 agents).
