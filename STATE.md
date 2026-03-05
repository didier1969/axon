# État du Projet : Axon

## Référence Projet

**Vision :** Axon est un **Copilote Architectural** qui permet aux agents IA et aux développeurs de comprendre, naviguer et auditer n'importe quelle base de code via une intelligence structurelle profonde.
**Focus Actuel :** v0.9 Couverture Langage & Intelligence sémantique.

## Position Actuelle

Milestone : **v0.9 Language Coverage**
Phase : 3 de 5 (Language Parsers — Part 1)
Plan : Validé (Java, C#, Ruby + Elixir Alias Resolution)
Status : 🟢 Prêt pour implémentation.

## Loop Position

```
PLAN ──▶ APPLY ──▶ UNIFY
  ○        ○        ○     [Phase 3 — ready to IMPLEMENT]
```

## Décisions & Évolutions

| Décision | Phase | Impact |
|----------|-------|--------|
| **Copilote Architectural** | v0.9 | Rebranding et changement de paradigme : du "où" vers le "comment/pourquoi". |
| **Backend HydraDB (v0.10)** | v0.9 | Planifié pour supporter l'échelle massive et le versionnage Dolt. |
| **Résolution Alias Elixir** | v0.9 | Identifiée comme correction critique pour la fiabilité du "Find Usages". |
| **Daemon obligatoire** | v0.8 | L'utilisation du daemon avec cache LRU est devenue la norme pour la performance. |

## Gap Analysis

### Daemon Crash
- **Status :** ✅ RÉSOLU. Daemon relancé (PID 59063). Fichiers stale nettoyés.
### Documentation racine
- **Status :** ✅ RÉSOLU. README, CLAUDE.md, GEMINI.md, ROADMAP mis à jour.
### Cahier des Charges HydraDB
- **Status :** ✅ RÉSOLU. Document `DOCS/HYDRADB_SPEC.md` créé.

## Prochaine Action

Commencer l'implémentation de la **Phase 3** (Résolution d'Alias Elixir + Parsers Java/C#/Ruby).
