# Session 140 — le gardien que je n'avais pas joué

2026-09-06 · HEAD `da85dae5` · servi `v0.8.0-1727-g8ade669a` · 17 commits en attente du promote

## Ce qui a été livré

Le plan token de la session 139 était complet à l'ouverture de cette tranche. Deux correctifs
de suite s'y sont ajoutés, chacun né d'une relecture plutôt que d'une nouvelle idée.

### `7ab37eb9` — `rows_rendered` voulait dire deux choses

`borner_lignes_sql` rendait un `usize`. Sur le chemin « sortie non délimitable » il valait **0**,
exactement comme sur le chemin « la borne n'a pas mordu ». Le client recevait alors
`status: ok_truncated`, `row_count: null`, `rows_rendered: 0` — un triplet qui se lit « bornée à
zéro ligne » alors que du texte **est** rendu. C'est la confusion que `ok_empty` existe déjà pour
éviter, réintroduite trois commits plus loin par un champ neuf.

Le retour passe à `Option<usize>`, et le statut distingue les deux coupes :

| `status` | `row_count` | `rows_rendered` | sens |
|---|---|---|---|
| `ok` / `ok_empty` / `ok_uncounted` | total ou `null` | `null` | rien n'a été coupé |
| `ok_truncated` | total | `n` | `n` lignes rendues sur le total |
| `ok_truncated_undelimited` | `null` | `null` | coupé à plat, non comptable |

Le mapping est sorti en fonction pure `statut_apres_borne` : c'est le seul endroit où les trois
champs se rencontrent, et `axon_sql` n'est pas exerçable sans base.

Mesure inscrite pour que la constante 60 000 ne se relitigue pas : sur 135 529 appels `sql`, les
heures contenant un appel au-dessus du seuil portent **0,78 % des appels** (majorant — le rollup
est horaire) et **~100 % des octets**. Moyenne par appel : 2 190 o, soit 3,6 % du seuil. La borne
touche moins d'un pour cent des appels et récupère la quasi-totalité du volume.

### `da85dae5` — j'avais laissé le gardien du dépôt rouge

`5c7218cd`, quatre commits plus tôt, avait ajouté deux outils à `DECLARED_DISPOSITIONS` en ne
déclarant qu'une partie de leurs paramètres : `retrieve_context` **3 sur 10 servis**,
`soll_work_plan` **2 sur 11**. Le test `runtime_surface::toute_disposition_declaree_couvre_exactement_le_schema_de_son_outil`
était rouge sur HEAD depuis, et personne ne l'avait vu — moi le premier.

Pourquoi je ne l'ai pas vu : j'avais joué `cargo test --lib -- tool_contracts`, le filtre du module
que je venais d'écrire. Ce fichier teste une **inclusion** — « ce que je déclare existe au schéma » —
qui reste verte quand on déclare trop peu. Le gardien teste une **égalité**, et il vit sous le nom
de la *surface*, pas sous celui de la feature. Un vert local était parfaitement compatible avec un
rouge global, et c'est ce sens-là qui trompe.

Le correctif ne pouvait pas être de compléter les 16 propriétés manquantes en `Honoured` : ç'aurait
été affirmer avoir lu seize handlers sans en ouvrir un, c'est-à-dire produire exactement la fiction
que `REQ-AXO-902583` combat. `ToolDispositions` sépare donc deux choses de natures différentes :

- `declared` — les paramètres dont le handler a été **lu** ;
- `unexamined` — les propriétés que personne n'a encore lues.

`unexamined` n'est délibérément **pas** une variante de `ParameterDisposition`. Une disposition dit
ce que l'outil fait du paramètre ; ce champ dit l'état de notre connaissance. Dans l'énumération, il
se lirait « ce champ est sans effet » alors qu'il dit « personne n'a regardé ». Le chokepoint ne lit
que `declared` : on ne signale jamais comme inerte un paramètre dont on n'a pas lu le handler.

L'invariant d'égalité exacte tient toujours — `declared ∪ unexamined` couvre le schéma, sans
chevauchement — mais la queue non lue coûte une ligne au lieu d'un mensonge, et la dette devient
comptable.

Deux détails de méthode valaient d'être posés. La règle sort en fonction pure `ecart_de_couverture`,
que le gardien **appelle** : sans cela le mutant aurait éprouvé une copie, pas le code qui garde
réellement le dépôt. Et un second plancher, `PLANCHER_PARAMETRES_EXAMINES`, parce qu'`unexamined`
ouvre un vecteur de triche évident — un outil entièrement non examiné ajoute 1 au compte d'outils
et 0 à l'honnêteté.

## Trois mesures qui ont réécrit le plan avant qu'il commence

Le plan de la suite (`~/.claude/plans/mutable-tinkering-lighthouse.md`) a été préparé par des
sous-agents en reconnaissance. Trois de leurs relevés ont contredit ce que la SOLL affirmait.

**`REQ-AXO-902625` n'est pas une passe large.** Le nœud annonçait ~20 sites d'appel à sérialiser et
demandait une autorisation d'édition en conséquence. La lecture du code donne autre chose : un
*lost update* dans `IstSnapshotCache::publish`/`evict` (`cache.rs:70-85`), read-modify-write non
atomique sur la carte entière. Deux écrivains sur des projets **disjoints** s'effacent quand même,
parce que le `store` final remplace toute la carte. Correctif : `ArcSwap::rcu`, six lignes, un
fichier, zéro test modifié. Et le partenaire de course n'était pas entre les trois tests RAM : c'est
`ensure_ram_snapshot_warm`, qui publie sans jamais évincer — donc une course de **production**, que
la passe large n'aurait pas vue.

**`REQ-AXO-902402` porte sur 7 lignes, pas 4 907.** Le nœud arbitrait entre une écriture directe sur
l'IST live (réservée à une demande explicite de l'opérateur) et un `rescan_project full` coûtant
110× plus cher. Mesuré ce jour : sept morceaux `file_context` hors fenêtre, sur six projets, tous à
512 jetons pile. L'indexation continue avait résorbé 99,86 % du problème seule — et avec les
nombres, le ratio de 110× qui justifiait de franchir la porte a disparu.

**Le bundle d'ouverture est plus gros que ce qu'on mesurait.** Les 103 315 caractères relevés sont
le `data` **seul** ; `content[0].text` s'y ajoute. Et il duplique les corps : le serveur le dit
lui-même dans sa propre prose (`workflow_project.rs:2829`) — bodies « INLINED in full in the
Continuation block ABOVE **and mirrored here** ».

## La leçon, en une phrase

Les trois cas ont la même forme. Un fait a été mesuré une fois, inscrit, puis relu comme s'il tenait
encore. Le gardien rouge, les 4 907 morceaux, la taille du bundle : à chaque fois, la seconde mesure
a coûté un appel et évité un chantier. Un REQ porte deux choses de durées de vie opposées — son
intention, qui reste vraie, et ses nombres, qui périment en silence pendant que le système tourne.

Corollaire pour l'opérateur, qui vaut d'être dit franchement : une autorisation est adossée à une
arithmétique. Celle de la passe large ne s'est pas consommée, le correctif faisant six lignes. Celle
de l'écriture IST live portait sur 4 907 lignes et un ratio de 110× — à sept fichiers, elle n'a plus
d'objet. Réutiliser un accord obtenu sur d'autres nombres, ce serait trahir le raisonnement qui l'a
produit.

## Ce qui reste bloqué

Le promote, et lui seul. **Dix-sept commits non servis** depuis `8ade669a`, dont `7f90710d` qui
laisse la queue de latence MCP à 1 Hz en production, et `06d553de` qui borne le corps du session
pointer — non servi, c'est pourquoi `re_anchor` a été refusé **deux fois** cette session, à 343 165
puis 347 469 caractères. Le pointeur a dû être résolu par `jq` sur la sortie sauvegardée.

Il y a là une boucle qu'il faut nommer : les correctifs qui rendraient les outils d'ouverture et de
recalage utilisables sont précisément ceux que le promote n'a pas encore servis.
