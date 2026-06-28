# Mémoire de bonnes pratiques cross-tenant (gouvernée, auto-améliorante)

> **SOLL :** `REQ-AXO-902131` (BELONGS_TO `PIL-AXO-002`).
> **Statut :** ✅ **LIVRÉ et LIVE** (build ≥ `v0.8.0-1280`). Les tools `practice_recall` /
> `practice_put` / `practice_tick` / `practice_card` sont câblés (`catalog.rs` / `dispatch.rs`)
> et servis par le brain ; encodage dense (902136), fusion sémantique (902137), consolidation
> épisode→règle→principe (902138), décroissance par périssabilité (902141) et partitionnement
> multi-agent scope/role/model (902149) sont LIVRÉS. Migration des `feedback_*.md` → `practice_*`
> faite (REQ-AXO-902146). **Source de vérité = `src/axon-core/src/mcp/tools_practice.rs` + le
> schéma catalog ; ce doc = contexte/rationale, pas le contrat.**
> **Origine :** convergence `DEC-NEX-008` — prototype Nexus généralisé (REQ-AXO-902131).

## 1. Thèse

Tous les projets ont besoin d'une **vraie mémoire de bonnes pratiques** : **structurée**,
qui **s'améliore** constamment et **se remplace** (oublie ce qui ne sert plus). Ce n'est
**pas** la mémoire conversationnelle d'un LLM, ni un RAG passif. C'est la frontière
« agentic memory governance » 2026 : entrées structurées + write-gate anti-poison +
décroissance + trust + méta-supervision, **partagées entre projets**, avec **Axon comme
centrale** (dogfood compris).

## 2. Pourquoi Axon (≈ 80 % du substrat existe déjà)

| Brique d'une mémoire SOTA | Déjà dans Axon |
|---|---|
| Entrées **structurées** | SOLL (intent versionné) / table PG dédiée |
| Rappel **sémantique** | `pgvector` + `retrieve_context_layered` |
| **Write-gate anti-poison** | `contradiction_check` (`REQ-AXO-902096`, scan EXACT `902129`) |
| **Partage inter-projets** | mailbox A2A (`REQ-AXO-902112`) |
| Annuaire des contributeurs | `project_registry` |
| ✅ **Décroissance FSRS + trust/renforcement + méta-moniteur** | LIVRÉ (`practice_memory.rs` : `decay_trust`/`retrievability`/`should_prune`, renforcement Physarum au recall, `assess_stagnation`) — REQ-AXO-902141/138/137 |

La surface neuve a été livrée par **composition** de l'existant (le « manque » Nexus est comblé).

## 3. Modèle de données (additif — nouvelle table, ne touche rien)

`practice` :

| champ | rôle |
|---|---|
| `id` | déterministe (`scope::context_sig::key`) → l'upsert fusionne au lieu de dupliquer |
| `scope` | projet propriétaire (+ flag `shareable` pour l'opt-in cross-projet) |
| `context_sig` | signature du contexte (généralise le rappel) |
| `context_embedding` | `pgvector` du contexte → rappel ANN |
| `practice` | le texte de la bonne pratique / le conseil |
| `evidence` | refs (commit, REQ, run) justifiant la pratique |
| `verdict` | `:reinforced` / `:provisional` / `:deprecated` |
| `weight` | trust courant (renforcement N4 / décroissance FSRS) |
| `last_used_at` | pour la décroissance |

## 4. Contrat MCP pressenti (4 tools, additifs)

- `practice_put(scope, context, practice, evidence)` — **write-gated** : passe d'abord
  `contradiction_check` ; rejet si la pratique **contredit le savoir indexé** (anti-poisoning).
- `practice_recall(scope, query, top_k)` — rappel `pgvector` scopé projet (+ cross-projet
  si `shareable`), trié par pertinence × `weight`.
- `practice_tick(scope)` — **décroissance** des inutilisées (FSRS-like) + **prune** des
  atrophiées ; **renforcement** des rappelées-utiles. La boucle « s'améliore / se remplace ».
- `practice_card(scope)` — résumé par projet (top pratiques, stagnation éventuelle).

## 5. Logique d'auto-amélioration — PROUVÉE côté Nexus (design de référence)

Portage direct des modules Nexus déjà testés (suite verte, hors-ligne, déterministe) :

| Axon | Module Nexus de référence | Commit |
|---|---|---|
| renforcement / décroissance / prune | `Learning.LessonWeights` (réutilise `ChannelWeights` N4) | `d947175` |
| écriture de pratiques depuis l'échec | `Learning.Extractor` (H7) | `b8d8db8` |
| rappel avant action | `Learning.Recall` (N1) | `b8d8db8` |
| **méta-moniteur** (détecte la stagnation → propose une mutation) | `Learning.MetaMonitor` | `d947175` |
| **preuve** « partie 2 > partie 1 » (série `[400→0→0] :improving`) | `Learning.Experiment` | `2499503` |

Le prototype Nexus a **démontré la boucle de bout en bout** : écrire → rappeler → éviter →
mesurer → renforcer/remplacer → s'auto-surveiller. Axon généralise ce pattern, agnostique
au domaine.

## 6. Dogfood — premier consommateur

**Nexus grandmaster** branche son store de leçons in-memory sur `practice_put`/`practice_recall`
d'Axon. Ça valide les deux d'un coup : la mémoire Axon fonctionne **et** les échecs gagnent
leur persistance. Un seul run live (quand la clé/Stockfish sont fournis) sert la généralisation.

## 7. Séquencement & non-disruption (important)

1. **Maintenant** (cette branche) : spec SOLL + ce design. **Zéro** touche à la surface chaude.
2. **Quand la file mailbox/NLI du coder se vide** : migration PG additive + module store +
   les 4 tools.
3. **Dernière étape seulement** : câblage `catalog.rs`/`dispatch.rs`/`tool_contracts.rs`
   (enregistrement des tools) — fait en un petit diff isolé pour **zéro conflit** avec le mailbox.
4. **Jamais** sur cette branche : promote-live, touche au brain en cours d'exécution.

Dépendances dures : `contradiction_check` (fix `902129` à re-vérifier) + mailbox A2A
(`REQ-AXO-902112`) atterris.
