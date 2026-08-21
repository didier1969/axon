# Session 122 — 2026-08-21/22 — l'outil de livraison annonçait le bon commit et en produisait un autre

Audit-only, append-only. Canonique = `CPT-AXO-052` + SOLL. Live à l'ouverture :
`v0.8.0-1541-gb5db4ed3`, HEAD `c50b4d7e`.

---

## 1. Le fil de la session : une surface qui affirme ce qu'elle n'a pas mesuré

Trois défauts livrés ou tracés ce jour disent la même chose sous trois formes.

| Surface | Ce qu'elle disait | Ce qui était vrai |
|---|---|---|
| `soll_validate` | « cette Decision n'a **aucun lien** » | elle en avait un, `REFINES`, légal, posé par l'outil d'écriture lui-même |
| `query` (résultat vide) | « use the guidance below » | aucune ligne de guidance dans le texte rendu |
| `axon_commit_work` | « Committed exactly the 1 declared path » | git venait d'en commiter deux |

Le troisième est le plus instructif parce qu'il a été **mesuré en direct** pendant la
falsification du correctif (§3).

---

## 2. `REQ-AXO-902405` — le validateur recopiait une part de la politique

`decision_without_links` codait en dur `SOLVES | IMPACTS`. La matrice de relations, elle,
admet `["SOLVES", "REFINES"]` pour `DEC → REQ`, avec `allow_multiple_types` — donc aucune
canonisation automatique ne rattrape le choix de l'appelant.

Conséquence : une Decision créée par le **chemin nominal**, avec la relation que
l'écrivain déclare légale, **naissait en violation**. Et le même rapport, quinze lignes
plus haut, conseillait en réparation « link each decision to a requirement with `SOLVES`
**or `REFINES`** ». Il reprochait d'avoir fait ce qu'il conseillait.

La règle lit désormais la politique. Le test **fait jouer les deux côtés ensemble** —
écrivain puis validateur — parce qu'un test qui n'interroge qu'un seul côté ne peut pas
voir une divergence entre les deux. C'est précisément ce qui a laissé celle-ci vivre.

---

## 3. `REQ-AXO-902417` — le commit prenait l'index entier

Signalé par **TE2** (`mcp_feedback` #186), confirmé en source primaire :
`workflow_project.rs:624` faisait `git commit -m <msg>`, sans pathspec.

Chez TE2 : `diff_paths` = 2 fichiers, deux autres stagés plus tôt dans la session pour un
commit ultérieur. Le commit produit en contenait **4**, dont une suppression de 401 lignes
que le message ne mentionnait pas.

**L'ironie mesurable** : la même fonction porte *deux* gardes soigneuses contre un
pathspec absent au moment du `git add` — REQ-AXO-902169 (refus d'un `diff_paths` vide) et
le saut du `git add` quand rien n'est stageable, toutes deux motivées par « sans pathspec
ça balaierait l'arbre ». **Zéro garde sur le commit**, cent lignes plus bas, même défaut.

### Ce que la falsification a montré, et qui vaut plus que le correctif

Les quatre tests sont passés au vert du premier coup. En remettant la ligne fautive :

```
  commit contained: "declared.txt\nunrelated.txt\n"
  tool said: ... Committed exactly the 1 declared path(s): `declared.txt`
             NOT committed: `unrelated.txt`
```

**L'outil annonçait déjà le bon résultat pendant que git en commitait un autre.** Un test
qui aurait lu la prose de l'outil — le réflexe naturel, c'est ce que la réponse MCP rend —
serait passé au vert sur l'outil cassé. Les tests interrogent `git show --name-only`.

### Le trou trouvé par la relecture, pas par les tests

Quatre tests, et **aucun ne faisait passer un fichier NEUF** par le commit borné : tous
déclaraient des chemins que git suivait déjà. C'est pourtant le cas le plus fréquent de
l'outil (tout nouveau fichier de test, tout nouveau module), et la formulation de git pour
un commit limité par chemin est *« record the current content of the listed files
(**which must already be known to Git**) »*. Que le `git add -A --` suffise à rendre un
fichier « connu » était **raisonné, pas mesuré**. Mesuré depuis : ça marche. Mais l'écart
entre les deux est exactement ce que cette session passe son temps à rattraper.

### Décision explicite : ne pas afficher « déclaré mais inchangé »

La réponse nomme les chemins **réellement** commités (mesurés contre l'index), pas
`diff_paths`. J'ai écrit puis retiré une seconde liste « déclaré mais sans changement » :
la dériver demande de comparer `declared` — qui peut être un **répertoire** — aux chemins
de fichiers de git, par chaîne. C'est le genre de recopie à la main qui a produit le
défaut §2. Chaque ligne affichée est mesurée, ou n'est pas affichée.

### Corollaire : une enveloppe qui mentait aussi

Un `git commit` en échec revenait dans une enveloppe **de succès**, ouvrant sur
« Validation passed », sans `isError`. Borner le commit rend deux modes d'échec
atteignables qui ne l'étaient pas (merge en cours ; chemins déclarés sans changement) —
laisser l'enveloppe telle quelle aurait été une régression de visibilité. L'échec est
désormais classé, et le cas merge **ne retombe jamais** sur le commit total : ce serait
rétablir le défaut en silence, sous la seule condition où l'index contient à coup sûr du
travail non déclaré.

---

## 4. Relève des canaux — les deux, toujours

`mcp_inbox_read` : **0 message**. `mcp_feedback_report` : **18 ouverts, 5 nouveaux**.
La leçon de la s121 tient : vérifier un canal et répondre pour les deux produit un fait
négatif faux.

Cinq doléances TE2, toutes vérifiées par leur auteur avant envoi, toutes tracées :

| TE2 | REQ | Note |
|---|---|---|
| 186 `axon_commit_work` index entier | `902417` P1 | **confirmé en source, livré** |
| 185 `soll_attach_evidence` enum ≠ accepté | `902418` P2 | `commit` accepté, absent de l'enum |
| 184 `mcp_inbox_read` volume vs `limit` | `902419` P2 | jumeau de VPC #181 |
| 183 `debt_digest` section `dry` | `902420` P1 | **0 actionnable sur 15** |
| 182 `wiring` faux positif `test_only` | `902421` P2 | 1 sur 10, non rejoué chez nous |

**#182 mérite d'être lu pour la méthode** : TE2 a vérifié les 9 autres `test_only` avant
d'écrire et n'en signale qu'un, en nommant les 5 sur lesquels l'outil a raison et les 3
qui relèvent d'une limite connue (dispatch dynamique). Sans ce travail, un correctif de
masse partait sur un signal faux.

**#183 est le signal commercial de la journée** : `debt_digest` annonce 2242 paires
`dry` ; sur les 15 « plus centrales », **zéro actionnable** — dix sont des `fused_L*`,
artéfacts d'indexation que personne ne peut ouvrir. Ce qui a marché à la place tenait en
trois lignes de shell et a trouvé trois vrais problèmes, dont trois modules `RateLimiter`
coexistants où **le seul supervisé n'est pas celui que la documentation destinée à un
régulateur documente**.

---

## 5. Trouvé en passant

`REQ-AXO-902422` — la **première ligne** de chaque init annonce
« Session pointer : CPT-AXO-052 — **Session 113 close** — live v0.8.0-1493 » alors que le
corps du nœud, lu à la seconde d'après, dit « Session **121** close », live
`v0.8.0-1541`. Le `label` est une copie figée à l'enregistrement du pointeur ; le nœud
bouge à chaque handoff, la copie non. **Huit sessions d'écart sur le signal d'orientation
le plus lu du produit** — et l'étiquette n'est pas fausse, elle était vraie en s113, ce
qui la rend coûteuse : elle se lit comme fraîche.

---

## 6. Outillage opérateur

`/relais` (`~/.claude/commands/relais.md`) — handoff GUI-PRO-028 **vérifié** puis compact.

Le point qui a demandé une correction : j'avais écrit que le hook post-compact « rejoue
l'init GUI-PRO-102 ». **Faux.** Lecture faite de `axon-reanchor.sh` : il injecte un rappel
d'une cinquantaine de jetons, délibérément — un bloc statique complet avait été rejeté
comme taxe par compact. Ce que ce rappel fait de décisif, en revanche, c'est dire qu'une
instruction placée dans l'**argument** de `/compact` n'a pas été exécutée et doit l'être.
C'est donc l'argument qui transporte « axon init puis reprends » par-dessus le compact —
mécanisme vérifié cette session même, puisque c'est ainsi qu'elle a repris.

Une commande qui décrit un hook sans l'avoir lu aurait été le défaut §1, appliqué à
l'outillage.

---

## Chiffres

| | |
|---|---|
| Porte à l'ouverture | `--lib` 1887/0 (7 ignorés) · `--bins` 43/0 · `cargo build --tests` |
| REQ livrés | `902405`, `902407` (`b5bbeb37`) ; `902417` en cours |
| REQ tracés | `902417` → `902422` (6) |
| Doléances relevées | 18 ouvertes, 5 neuves (TE2), 1 `blocking` (VPC #181) |
