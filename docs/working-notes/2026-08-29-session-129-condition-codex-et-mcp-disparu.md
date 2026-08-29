# Session 129 — le MCP disparu, et la couche d'instructions ramenée à la condition Codex

**Date** : 2026-08-29 · **Branche** : `main` · **Déclencheur** : « axon init », puis question opérateur sur l'écart de rigueur entre Claude et Codex.

## Ce qui a été mesuré

### Le MCP n'était plus monté côté Claude

Cette session a démarré avec **zéro outil `mcp__axon__*`**. Tout le début du travail est passé par `curl` sur `http://127.0.0.1:44129/mcp` — donc hors gates, hors trace.

Cause racine : la CLI Claude Code a migré son stockage de `~/.claude.json` vers `~/.claude/.claude.json`. L'ancien fichier conservait l'entrée `mcpServers.axon`, ce qui donnait l'illusion d'une configuration correcte, mais la CLI lit le nouveau, où elle était absente.

Fausse piste écartée en cours de route : `~/.claude/settings.json` porte `"mcpServers": {}`. Ce n'est pas la cause — après réparation, son md5 est resté identique.

Le serveur n'était pas en cause : 114 outils conformes (noms, `inputSchema`), servis en stdio **et** en HTTP. Le test discriminant `tools/list` via le pont stdio rend 114 outils, exit 0, stderr vide.

→ `REQ-AXO-902552` (delivered).

### L'écart de rigueur Claude / Codex est réel, et plus modeste que le premier chiffrage

Première mesure erronée : elle comparait Codex sur **tout le parc** à Claude sur le seul dépôt axon, et sur deux périodes différentes. Corrigée en filtrant le `cwd` des rollouts Codex.

À périmètre égal, par `axon_commit_work` :

| | Claude | Codex | écart |
|---|---|---|---|
| `soll_query_context` | 0,12 | 1,28 | 10,4× |
| `soll_attach_evidence` | 0,44 | 1,74 | 3,9× |
| `impact` | 0,07 | 0,31 | 4,2× |
| `axon_pre_flight_check` | 0,63 | 1,10 | 1,7× |
| `inspect` | 0,83 | 0,82 | **1,0×** |
| `sql` | 4,15 | 0,69 | 0,2× |

La navigation structurelle est à parité. Le déficit porte sur la **cérémonie et la preuve**.

**Rétractation** : un premier chiffrage annonçait « 6,6× plus d'escape hatch ». Classification des 491 appels `sql` de Claude : **46 seulement** touchent le motif interdit (`soll.node` + `description`) ; les 445 autres sont de l'analytique IST, `soll.edge`, mailbox, `information_schema` — du travail qu'aucun autre outil ne fait. Les 27 appels `sql` de Codex n'ont pas été classés, donc l'écart sur le motif réellement interdit n'est **pas** établi.

### La cause n'est pas le modèle

Le `sql` de Claude par jour : 17,9 % → 20,1 % → 6,6 % → 20,9 % → **35,3 %** → 16,5 % → 29,9 % → **0 % → 0 %** les 25–26/08. Même modèle, même client, même semaine. Un trait de modèle ne produit pas ce virage ; un changement de contexte, si.

### Ce qui différait vraiment

Le skill `axon-engineering-protocol` est **byte-identique** des deux côtés (md5 `c3affce3bcd8`). La couche autour, non :

| | Codex | Claude (avant) |
|---|---|---|
| Skills valides | 12 (97 des 109 liens étaient morts) | 27 + plugins |
| Instructions | 3 389 o | 198 517 o |
| Prescrit | une **séquence** | une **vitesse** (« one burst », cache-TTL) |

Grep `burst\|cache-TTL\|token\|cost\|speed` côté Codex : **zéro occurrence**.

### La surface méthode d'Axon existait, et dormait

30 nœuds `Skill` (8 réels), 26 `PromptTemplate` (**25 de pollution**, seul `PRT-PRO-999` était réel).
Usage : `skill_invoke` 2 appels côté Codex, **0 côté Claude** ; `prompt_template_get` **0 partout, jamais**.

Six skills fichier avaient un jumeau SOLL. Vérification avant archivage : **4 des 6 copies fichier étaient 1,5 à 3,2× plus grosses que leur jumeau** (`diagnose` 7 163 o contre 2 220 o). Ce n'était donc pas une duplication propre → `REQ-AXO-902553`.

## Ce qui a été fait

1. **MCP réparé** — `claude mcp add axon -s user` → `axon ✔ Connected`.
2. **6 guidelines créées** (`GUI-AXO-1034`→`1039`) : porte de build, sonde d'hôte, devenv shell, promote seul chemin vers `bin/`, politique de données, pas de `pkill` large. Elles ne vivaient que dans `CLAUDE.md`.
3. **20 `PRT-PRO-*` pollués passés en `rejected`** — la passe d'hygiène avait été faite sur `Skill` et n'avait jamais atteint son jumeau `PromptTemplate`. Aucune violation créée.
4. **`PRT-PRO-999` complété** puis rendu en trois fichiers d'amorçage de 2 818 o : `CLAUDE.md`, `AGENTS.md` (**inexistant jusqu'ici**), `.gemini/system.md`.
5. **290 819 o d'instructions archivés** (3 `CLAUDE.md`, `MEMORY.md`, 104 fichiers mémoire) vers `~/.claude/archive-2026-08-29-condition-codex/`. Nouveau global : 1 602 o.
6. **Skills ramenés à 12**, identiques à ceux de Codex. Plugins désactivés. 97 liens morts de `~/.codex/skills` retirés, inventaire conservé.
7. **`scripts/measure-agent-discipline.py`** — reproduit les mesures manuelles au chiffre près.

## Baseline pour la suite

Le succès se lira sur `soll_query_context`, `soll_attach_evidence` et `axon_pre_flight_check`, sur **≥ 3 sessions de travail réel**, jamais sur une seule. Indicateur secondaire : `skill_invoke` et `prompt_template_get` doivent quitter zéro, sinon la surface canonique reste morte.

## Confondants assumés

- Les 15 hooks de `~/.claude/hooks/` sont conservés : Codex n'en a aucun, mais ceux-ci poussent *vers* la méthode.
- Les fenêtres de mesure Claude et Codex ne se recouvrent pas (Claude 18–26/08, Codex 26–29/08) : les deux agents ont travaillé en séquence, pas en parallèle. Aucune comparaison à tâche identique n'est possible sur les données existantes.

## Reste ouvert

- `REQ-AXO-902553` — 4 nœuds `SKI-PRO-*` plus minces que la copie archivée.
- `REQ-AXO-902554` — 18 arêtes `INHERITS_FROM` vers un projet `CHC` inexistant, qui masquent les 8 vraies violations `PRO`.
- `REQ-AXO-902555` — la télémétrie MCP ignore quel client appelle.
- ~~Décision opérateur en attente sur `PRT-PRO-994`→`998`~~ — **tranchée** : ces cinq étaient eux aussi des fixtures d'une ligne (`Slug: {{slug}}.`, `Run {{iterations}} times.`), rejetés sur instruction. Nettoyage final : **25 des 26 `PromptTemplate`**, seul `PRT-PRO-999` reste `current`.
- `REQ-AXO-902556` — `practice_put` corrompt la mémoire gouvernée (voir section suivante).

## Après-coup — vérifications, nettoyage parc, et un défaut trouvé au handoff

**Le nouveau fonctionnement est vérifié, pas supposé.** Deux tests sur processus neuf :
- CLAUDE.md projet = le nouveau · pas de section cache-TTL · pas de bandeau REMEMBER · pas de superpowers · 1 seul skill `axon-*` · 114 outils `mcp__axon__*` montés.
- « axon init » route vers `GUI-PRO-102` **sans** l'ancien CLAUDE.md, et annonce les 3 bons premiers appels (`status mode=brief` → `axon_init_project` → `practice_recall`). La mitigation prévue au plan tient.

Réserve : `/clear` seul ne suffit pas — le catalogue de skills et les plugins sont chargés au lancement du binaire. Il faut quitter et relancer.

**27 répertoires `.remember` supprimés (~18 Mo).** Résidus du plugin `remember`, désactivé ce soir ; journal parallèle à Axon, sans garde anti-contradiction ni preuve pointée. Il avait d'ailleurs enregistré le « 6,6× » rétracté et désigné `settings.json` comme cause — il empile, il ne se relit pas.
Le premier balayage (`-maxdepth 2`) en avait trouvé 24 ; le balayage complet en a révélé 27, dont le plus gros du parc (3,6 Mo à `Fiscaly/elixir/`) et un hors arborescence (`~/.remember`). Vérifié avant suppression : 0 versionné sur 27, tous gitignorés, aucun hook actif n'y écrivait.

**`REQ-AXO-902556` — défaut trouvé pendant le handoff lui-même.** `practice_put` inline `dense` et `evidence` dans le corps du champ `practice` : le texte stocké porte littéralement `</practice><parameter name="dense">`, `encoding` retombe en `prose_fallback` alors que `dense` a été fourni, et l'`evidence` peut finir vide. Reproduit 3 fois de suite, y compris en tentant de contourner. Ce n'est pas une faute d'appelant : **3 pratiques antérieures du parc portent le même défaut** — `1222` (KKI), `1249` (APS3D), `1274` (OPV), écrites par des sessions et des tenants différents. La mémoire gouvernée étant primaire à l'init, la corruption est relue par chaque session de chaque projet.
Doublons créés en tentant de corriger : `1701` et `1703` retirés par `practice_retire` (jamais supprimés), `1702` conservée.

**Session pointer** : `CPT-AXO-052` pèse **77 739 o**. La pratique 1639 du parc documente le mode de ruine exact à 94 Ko — `soll_get` finit par refuser de rendre le corps, et ~25 000 jetons sont consommés à chaque réveil. Le titre a été corrigé (il annonçait encore « session 124 »), mais la réécriture du corps est une opération structurante qui n'a pas été engagée sans l'opérateur.

**Portes finales** : `soll_validate(AXO)` 0 violation · couverture 1230 done / 65 partial / 2 missing (les 2 sont préexistants : `REQ-AXO-902072`/`902073`) · arbre propre · 0 commit non poussé.
