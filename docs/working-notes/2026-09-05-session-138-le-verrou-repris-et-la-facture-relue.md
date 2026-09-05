# Session 138 (2026-09-05) — le verrou repris, et la facture qu'on relit

> Note de travail, append-only. **La SOLL fait foi** : l'état vivant est dans `CPT-AXO-052`.
> Cette note garde le *raisonnement*, pas l'état.

## Ce qui a été livré

Trois commits, porte `GUI-AXO-1034` verte — `bins=0 lib=0 testbins=0 buildtests=0`,
**2 156 passed / 0 failed / 8 ignored** en 735 s.

| commit | REQ | fait |
|---|---|---|
| `42b29d42` | `902614` | reprise du verrou IST entre frères du même superviseur |
| `06ed86a9` | `902616` | `indexer_alive` dit QUEL indexeur vit ; le gate cesse d'expliquer faux |
| `c9b2d74f` | `902618` | la santé de `ist_writer` entre dans `truth_status` |

## ⭐ Le remède prescrit n'existait pas

`REQ-AXO-902614` demandait qu'un démarrage refusé « sorte avec un code distinct, et que
`restart: on_failure` cesse de le relancer ». J'ai lu les politiques dans le binaire déployé :

```
RestartPolicy ∈ always | on_failure | exit_on_failure | no
+ max_restarts, backoff_seconds, exit_on_end, exit_on_skipped
```

**Aucune condition sur le code de sortie.** `on_failure` relance sur *tout* code non nul, sans
exception possible. Le critère était irréalisable sur ce superviseur — pas mal écrit, simplement
fondé sur une capacité que `process-compose` 1.94.0 n'a pas.

Restaient `exit 0` (réfuté en session 137 : le processus passe `Completed` et n'est **plus jamais**
relancé) et `max_restarts` (casse l'intention de `REQ-AXO-902576`). Donc : **la boucle se casse en
faisant que le refus n'ait pas lieu.**

Le fait qui a tranché le design n'est pas la boucle, c'est la **sûreté** : le 04-09,
`process stop axon-indexer` a rendu `Successfully stopped` **sans tuer l'indexeur qui travaillait**.
Tant que le processus suivi n'est pas celui qui tient le verrou, arrêter le service ne l'arrête pas.
D'où la règle : *celui que le superviseur suit doit être celui qui travaille.*

Cinq gardes cumulatives avant toute reprise — et chacune protège un cas réel, pas un cas imaginé :

| garde | ce qu'elle empêche |
|---|---|
| reprise activée | l'opérateur garde `AXON_IST_WRITER_TAKEOVER=0` |
| identité déclarée ≠ `unknown-runtime` | deux `unknown` ne prouvent pas un même rôle — c'est l'absence de preuve |
| même identité runtime | un indexeur `dev` ne touche pas un `live` |
| même ppid | un indexeur lancé à la main n'est pas à nous |
| propriétaire plus ancien | le superviseur ne garde que la référence la plus récente |

## ⭐⭐ `kill(pid, 0)` réussit sur un zombie — et son propre test l'a attrapé

`terminate_and_wait` détectait la mort du propriétaire par `libc::kill(pid, 0)`. Le test
d'intégration à deux processus a échoué **après 120,06 s** pour un timeout de 120 000 ms.

La signature est parlante : *l'attente prend exactement la valeur du timeout, à la milliseconde*.
Cause : le propriétaire est un **enfant**, il reste zombie tant que son parent n'a pas appelé
`wait()`, et `kill(pid, 0)` **réussit sur un zombie**. L'attente ne voyait donc jamais la mort.

C'est exactement l'erreur que `REQ-AXO-902157` avait corrigée côté bash — `[ -e /proc/$pid ]`
répond vrai pour un zombie — refaite en Rust, dans le fichier qui porte ce commentaire.

Correctif : **sonder la ressource, pas le processus.** Retenter `flock(LOCK_EX|LOCK_NB)` sur notre
propre descripteur. Le noyau libère le flock à la mort du propriétaire, zombie inclus : c'est la
seule autorité. 26 tests verts, cas de reprise en 20 s au lieu de 120.

Pratique `2149`.

## ⭐ `REQ-AXO-902556` a enfin sa règle exacte

En déposant la pratique ci-dessus, j'ai reproduit la corruption **trois fois de suite**.

| id | `practice` en dernier ? | verdict |
|---|---|---|
| 2147 | non | corrompue, `evidence` = 0 |
| 2148 | non | corrompue, `evidence` = 0 |
| 2149 | **oui** | propre, `practice` 738 / `evidence` 248 |

**Tout paramètre émis APRÈS `practice` est absorbé dans son corps et perdu.** `dense` n'y est pour
rien — la session 137 avait vu la corrélation, pas la règle. Cela explique le chiffre déjà mesuré :
351 des 475 pratiques corrompues ont `evidence` VIDE, parce que `evidence` suit `practice` dans
l'ordre naturel du schéma.

La cause est l'**émission** de l'appel, pas le serveur. Le remède tient en un changement d'ordre.
2147 et 2148 retirées avec `superseded_by`. Pratique `2155`.

## ⭐⭐ La facture token, mesurée au lieu d'être estimée

Question de l'opérateur : « on a le potentiel de réduire notre facture token, de quelle magnitude ? »

Les transcripts portent l'usage réel par requête. Sur **15 078 requêtes / 11 sessions** :

| poste | part du coût |
|---|---|
| **relecture de cache** | **78,6 %** |
| écriture de cache | 11,8 % |
| sortie | 9,6 % |
| entrée non cachée | 0,0 % |

Le cache marche — 98,8 % de hits. **C'est le mécanisme du problème, pas sa solution** : chaque
requête relit **447 352 tokens**. On ne paie pas ce qu'on ajoute, on paie ce qu'on **relit**, autant
de fois qu'il reste de tours.

Composition des 22,7 Mo relus :

| catégorie | part | moyenne |
|---|---|---|
| **arguments envoyés aux outils** | **38,4 %** | 1 120 car |
| résultats MCP Axon | 26,5 % | 2 687 car |
| résultats Bash | 21,8 % | 1 031 car |

Le résultat contre-intuitif : **ce qu'on envoie pèse plus que ce qu'on reçoit.** Les corps SOLL
réexpédiés en entier dominent — alors que `soll_manager action=append_section` existe depuis
`REQ-AXO-902161` précisément pour ne pas les renvoyer.

Distribution des résultats Axon : médiane **368** caractères, p90 **25 623**, et **11 % des appels
portent 72 % du volume**. Loi de puissance : tout le gain est dans la queue, rien dans la médiane.
Optimiser « tous les appels » ne rapporterait rien.

Levier chiffré sur un cas réel : `axon_init_project` rend 108 000 caractères ≈ 27 000 tokens **en
ouverture** de session. Relu ~1 300 fois → **≈ 4,5 % de la facture d'une session pour un seul appel.**

Et le trou de gouvernance : `axon.mcp_call_stat` porte `call_count`, `latency_sum_ms`,
`latency_max_ms` — **aucune colonne d'octets**. On pilote la latence et on ignore le coût, alors que
`REQ-AXO-901934` pose « les tokens-en-sortie SONT la fonction de coût ». C'est pour ça que le pas 1
du chantier est *mesurer*, pas *optimiser*.

Réduction estimée : **25 à 35 % de la facture**. Ce qui est mesuré : la répartition, la composition,
la loi de puissance, le cas `init_project`. Ce qui est estimé : les facteurs de compression.

## Ce que la session dit de la méthode

**Deux défauts sur trois ont été trouvés par un test, pas par relecture.** Le zombie, et
l'irréalisabilité du critère 1 (trouvée en lisant le binaire plutôt qu'en croyant le REQ).

Le troisième — la corruption `practice_put` — a été trouvé en la **reproduisant sous mes doigts**
alors que je déposais une pratique *sur une autre leçon*. Une session 137 entière avait mesuré la
corrélation sans trouver la règle ; trois essais consécutifs l'ont donnée.

Corollaire de méthode : **un contrôle qui sait échouer vaut mieux qu'une relecture attentive.**
Le test d'intégration à deux processus n'a pas été écrit pour trouver le zombie — il a été écrit
parce que le critère 4 du REQ l'exigeait. Il a trouvé autre chose.

Trois frictions d'outillage, consignées pour ne pas les re-découvrir :
- un job nexus coupé par son propre `--timeout` se lit **exactement** comme un test suspendu ;
- `pkill -f <motif>` a tué mon propre shell — `GUI-AXO-1039` interdit le `pkill` large, je l'ai
  enfreint et j'ai payé dans la seconde ;
- insérer du code juste avant un `#[derive]` détache le derive de son type — trois erreurs `E0119`
  en cascade, aucune ne nommant la cause.

## Ouvert à la clôture

**Promote non fait, délibérément** : il coupe les clients, et l'enquête VPC
`msg-e6e6bcb8fceec5ebfa34f355` — 2 106 avis de promote/jour contre 14 pour le deuxième projet —
reste sans réponse de l'opérateur. Le binaire promu porte encore l'ancien A3 et aucune des trois
corrections. `axon-indexer` est `Completed` **volontairement**.

Chantier suivant arrêté et approuvé : **B1** (colonnes d'octets) → **B2** (double émission
`content`/`structuredContent`) → **B3** (squelette au-dessus de 4 000 caractères) → **B4**
(`token_budget`) → **B5** (ce qu'on envoie). Plan : `~/.claude/plans/sequential-stargazing-token.md`.

---

## Suite de session — le chantier token, et ce que la mesure a corrigé

### Livré

| commit | REQ | contenu |
|---|---|---|
| `189346c8` | `902621` | le POIDS des appels MCP, à côté de la latence |
| `58f914a5` | `902619` | le bundle d'ouverture ne sert plus de nœuds morts |

### ⭐⭐ Deux prémisses à moi, réfutées par la mesure

**1. « La double émission `content` / `structuredContent` coûte un facteur 2 » — faux pour ce client.**

Mesuré sur 2 286 résultats Axon : **0 %** portent les deux canaux. Le client Claude Code reçoit
`content[0].text` — du markdown — et **jamais** `data.kickoff_bundle` ; le mot `"pillars"` n'apparaît
nulle part dans ce qu'il relit. Le code le disait déjà, en commentaire, à l'endroit exact que je
m'apprêtais à modifier : *« the PROJECT indexes … only ever reached `data.kickoff_bundle`, which the
Claude Code client does not expose to the LLM »*. Je l'ai lu **après** avoir mesuré de travers.

B2 ne réduit donc rien de notre facture. Il reste vrai pour un client qui lit le canal structuré —
c'est le signal FSF, porté par `REQ-AXO-902537`. **Requalifié P2, B3 passe devant.**

**2. Ma décomposition « 108 841 caractères, `pillars` = 42 % » portait sur le mauvais objet.**

C'était la réponse *complète* écrite dans `tool-results/` au dépassement de budget — pas ce qui entre
dans le contexte. Le vrai objet relu fait **30 232 caractères**. Deux objets, deux conclusions
opposées. Pratique `2168` : *décomposer ce que le CLIENT REÇOIT, jamais ce que le serveur produit.*

### ⭐ Le défaut que la bonne mesure a trouvé — et ce n'est pas une économie

En décomposant le markdown réellement relu, le bundle d'ouverture servait **en corps entier** :

| pilier | statut |
|---|---|
| `PIL-AXO-902` « Test Pillar » | rejected — stub « placeholder rejected session 64 » |
| `PIL-AXO-102` « New Pillar » | rejected |
| `PIL-AXO-006` | superseded |

…pendant que `PIL-AXO-9003` « Axon Two-Sided Identity » (**8 152 car**), `PIL-AXO-004`, `007`, `008`,
`009` étaient **évincés**, budget de 12 Ko atteint.

Cause : `push_bodies` ne filtrait aucun statut, alors qu'`index_current` — dix lignes plus bas, dans
la même fonction — filtre `current`. Le budget d'inline se dépense dans l'ordre des identifiants,
donc des nœuds **morts** passaient avant des nœuds **vivants**. Toute session s'ouvrait sur « Test
Pillar » et n'avait jamais le pilier d'identité du produit.

**Une surface qui sert faux, pas une surface trop bavarde.** Je cherchais des jetons, j'ai trouvé une
erreur de contenu.

### ⭐ Mon test était complaisant, et seul le mutant l'a dit

Le premier jet lisait le projet AXO du serveur de test — qui ne porte **aucun** pilier rejeté. Il
passait donc avec **et** sans le filtre : 2 verts dans les deux cas. Un test d'absence écrit sur un
jeu de données d'emprunt passe pour la mauvaise raison — ce qu'il cherche n'y est simplement pas.

Fixture réécrite avec ses propres nœuds `rejected` + `superseded` et un corps repérable : le mutant
rougit désormais les deux tests **sur assertion**. Pratique `2169`.

Corollaire, et c'est le même défaut que `REQ-AXO-902616` sous une autre forme : *un test qui ne peut
pas échouer coûte plus cher que pas de test, parce qu'il fait croire que l'invariant est tenu.*

### Ce qui reste du chantier token

Ordre **mesuré** sur la queue (appels > 20 000 car, poids cumulé) : `batch` **170 358**,
`soll_get` **79 173**, `mcp_inbox_read` **49 793**. Puis B4 (`token_budget`), B5 (ce qu'on envoie —
38,4 % du contexte relu, la moitié qu'on oublie).

`REQ-AXO-902621` a instrumenté `axon.mcp_call_stat` : le gain de chaque tranche suivante se lit dans
`mcp_telemetry_report sort="bytes"`, il ne s'estime plus.
