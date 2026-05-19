# Recommandations Claude Code — Optimisation contexte et coût

**Date** : 2026-05-08
**Statut** : actionnable, à appliquer

---

## Vision macro

Trois leviers réduisent les coûts/latence sans dégrader la qualité :
1. **Supprimer la pollution récurrente** (CARL, hooks injectifs)
2. **Utiliser le MCP Axon comme source structurée** plutôt que des lectures de fichiers
3. **Discipline de session** (handoff manuel, /clear entre sujets, sub-agents)

---

## Actions à appliquer

### 1. Désinstaller CARL ✂️

**Pourquoi** : 80 % de ses règles sont déjà dans le system prompt Claude Code natif. Injection à chaque message = ~25-50k tokens gaspillés / session longue.

**Migration** :
- Règles génériques globales → `~/.claude/CLAUDE.md` (chargé une fois)
- Règles spécifiques projet → `~/projects/axon/CLAUDE.md` (déjà existant, à enrichir)
- Workflows → Skills natifs Claude Code (auto-activation contextuelle)
- Préférences personnelles → `~/.claude/memory/`

**Économie attendue** : 25-50k tokens par session longue, ~1,50-3 $/session.

### 2. Privilégier le MCP Axon pour exploration code 🔬

**Au lieu de** : `Read` direct sur fichiers, `Glob`, `Grep` exhaustifs (qui pollue le contexte avec du brut).

**Faire** : appeler les tools MCP Axon — code déjà graphé et vectorisé.

| Besoin | Tool MCP Axon |
|---|---|
| Trouver un symbole | `query("symbol_name")` |
| Détail d'un nœud | `inspect("entity_id")` |
| Contexte autour d'un sujet | `retrieve_context(...)` |
| Impact d'un changement | `impact("entity_id")` |
| Pourquoi ce code existe | `why("entity_id")` |
| Flux de dépendance | `path(from, to)` |
| Risques structurels | `anomalies(...)` |
| Intent SOLL | `soll_query_context(...)` |

**Différence concrète** : un `inspect` retourne ~1-3k tokens ciblés, un `Read` de fichier complet retourne ~5-50k tokens dont 90 % non pertinents.

**Quand cela ne suffit pas** : si la freshness MCP est dégradée (`status` ne reporte pas `fresh + canonical`), bascule sur lecture directe en attendant un re-index. Sinon, **MCP first**.

### 3. Discipline de session 🎯

#### Avant une interruption prévue
```
/handoff      ← skill qui condense la session en doc de reprise
/clear        ← reset contexte
```
→ Au retour : "reprends depuis [chemin du handoff]" → repart en contexte propre, zéro re-prefill payant.

#### Entre sujets non liés
`/clear` direct. Pas de regret à avoir.

#### Pendant une tâche complexe
- Une question = un sujet. Évite d'enchaîner 3 questions dans un message → cascade d'actions et expansion.
- Précise le périmètre attendu : "réponse courte, focus sur X" / "ne lance pas de recherche, raisonne sur ce que tu sais".

### 4. Demander explicitement les sub-agents pour exploration large 🤖

Pour toute tâche d'exploration > 3 lectures de fichier ou > 2 recherches web :
> "Utilise un sub-agent Explore pour cette recherche."

Le sub-agent travaille dans un contexte isolé, renvoie 1-2k tokens de résumé au lieu de 30-50k de raw data dans ta session.

### 5. Optimiser le format de réponse côté assistant 📐

À demander explicitement quand pertinent :
- "Réponse en 5 bullets max"
- "Pas de tableau, juste la conclusion"
- "Vision macro d'abord, détails seulement si je redemande"

L'assistant tend par défaut à produire du contenu structuré verbeux (tableaux, sections). Le contraindre à être bref économise output tokens (75 $/M sur Opus).

---

## Vision micro — actions immédiates aujourd'hui

1. ✅ **Désinstaller CARL** : retirer les hooks `UserPromptSubmit` qui injectent `<carl-rules>`, retirer les fichiers `.carl/`
2. ✅ **Migrer les 9 règles GLOBAL** vers `~/.claude/CLAUDE.md` (une fois pour toutes)
3. ✅ **Audit `~/projects/axon/CLAUDE.md`** : il est déjà bon, peut accueillir les règles spécifiques projet sans surcharge
4. ⚠️ **Tester un workflow** sans CARL sur la prochaine session : vérifier que tu ne sens aucun manque
5. 🔄 **Habituer la pratique `/handoff` + `/clear`** avant chaque pause > 10 min

---

## Économie cumulée attendue

| Action | Tokens économisés / session | $ économisés (Opus) |
|---|---|---|
| Suppression CARL | 25-50k | 0,40-0,75 $ |
| MCP Axon vs lectures fichiers | 30-100k | 0,45-1,50 $ |
| /handoff + /clear sur pauses longues | 200-400k évités en re-prefill | 3-13 $ |
| Sub-agents pour exploration | 30-50k | 0,45-0,75 $ |
| Réponses ciblées (output) | 5-15k | 0,40-1,15 $ |
| **TOTAL par session active** | **~290-615k tokens** | **~5-17 $** |

Sur 100 sessions/mois : **~500-1700 $/mois économisés**, sans perte fonctionnelle.

---

## Ce qui reste à mon initiative côté assistant

- Privilégier `query` / `inspect` / `retrieve_context` MCP avant `Read`
- Lectures ciblées (`offset`/`limit`) si fichier nécessaire
- Web search uniquement si l'info ne peut être ni dans MCP ni inférée
- Sub-agents proactifs pour exploration large
- Réponses denses : macro → micro, pas de remplissage

À toi de me corriger si je dévie.
