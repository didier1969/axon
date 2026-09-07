# Session 145 — le mort qui portait un contenu

**2026-09-07** · HEAD `5a8c5f4b` · porte `GUI-AXO-1034` **VERTE** (2 261 / 0, cinq étapes `rc=0`)

## Ce que la session a fait

Fermé la **sixième** des sept instances du motif ouvert le 2026-09-06 — « une capacité existe, une
autorité placée en amont l'empêche d'agir, sans erreur ni log ». Un commit, `5a8c5f4b`, 5 fichiers,
+167 / −173.

`REQ-AXO-902634` posait une alternative binaire : **brancher** la politique de blocage des subtree
hints, ou l'**enterrer**. La mesure a montré qu'aucune des deux ne convenait seule.

## Le geste qui a tranché — différer deux structures, compter le delta

`.axon/capabilities.toml` redéfinit **deux** listes de segments de répertoires. Comparées ligne à
ligne, elles ne diffèrent que par **trois segments** :

| liste | autorité qui la lit | contenu |
|---|---|---|
| `ignored_directory_segments` | `classify_path` ← `is_noise_directory` ← `build_walker_from` — **vivante** | 27 segments |
| `blocked_subtree_hint_segments` | `classify_subtree_hint_path` ← chaîne morte | les mêmes **+ `pg_wal`, `_bmad`, `_bmad-output`** |

Puis compter, sur les données réelles, ce que ce delta laisse passer :

```sql
SELECT … FROM ist.indexedfile
WHERE path LIKE '%/_bmad/%' OR path LIKE '%/_bmad-output/%' OR path LIKE '%/pg_wal/%'
```

`_bmad` **608** · `_bmad-output` **10** · `pg_wal` **0** — les 618 sous OptiPlanner. (`pg_wal` rend
zéro non grâce à cette politique, mais parce que les segments WAL n'ont pas d'extension et tombent
sur le filtre d'extension.)

## Le mécanisme était mort AVEC raison — et il le disait lui-même

`tools_system.rs::axon_rescan_project`, étape 4 :

> « REQ-AXO-901893 (LEGACY FEED PURGE): enrol the subtree directly into the durable work queue.
> […] This replaces the old `pg_notify('axon_registry_changed')` → `registry_notify_listener` →
> `ingress_buffer` hop (**both ripped**). »

Le design d'origine (`docs/archive/plans/2026-04-02-ingress-buffer-design.md` § 5 : « directory
event → enqueue `subtree_hint`, then let the reducer/promoter decide ») n'a jamais été construit,
puis a été remplacé. `record_subtree_hint` n'a donc pas *disparu* : il n'a **jamais existé** comme
fonction Rust. Il ne vivait que dans deux descriptions d'outil rendues à l'utilisateur.

## Verdict SPLIT

| élément | verdict |
|---|---|
| `should_buffer_subtree_hint`, `classify_subtree_hint_path`, `path_has_blocked_subtree_hint_segment[_with_config]` | enterrés |
| `subtree_hint_cooldown_ms`, `subtree_hint_retry_budget` | enterrées — zéro lecteur, même indirect |
| descriptions publiques (`catalog.rs`, `tools_system.rs`) | corrigées : le chemin réel est **synchrone** |
| les 3 segments | **remontés à l'opérateur** → `REQ-AXO-902638` (blocked) |

Le mécanisme était mort avec raison ; **son contenu ne l'était pas**. Pratique gouvernée `2289`.

## La garde, et pourquoi elle n'est pas morte-née

`config.rs::la_politique_d_exclusion_de_repertoires_n_a_qu_une_liste_et_aucun_champ_orphelin` :
déstructuration **exhaustive** de `IndexingConfig` (pas de `..`), donc tout champ neuf casse la
**compilation** tant qu'il n'est pas nommé avec son lecteur de production ; plus un banc d'effet
différentiel prouvant que `ignored_directory_segments` change le verdict de `classify_path`.

⛔ Une garde qui aurait vérifié la *présence d'un lecteur* serait passée au **vert** sur l'arbre
cassé : `blocked_subtree_hint_segments` **avait** un lecteur — un lecteur mort. Falsifiée dans le
bon sens : `E0027` + `E0063` nommant les trois champs orphelins avant le correctif.

Les deux tests portés sur la chaîne morte ont été **remis sur l'autorité vivante** plutôt que
supprimés : ils prouvaient une règle correcte derrière une porte fermée.

## Deux corrections à des notes antérieures

1. **« 28 commits attendent le promote » est faux** — `git rev-list b53078ee..HEAD --count` = 3
   avant cette session, 4 après. Le promote de s143 avait bien servi `b53078ee`.
2. **« `classify_subtree_hint_path` a un appelant de production »** était vrai littéralement et
   trompeur en fait : cet appelant était lui-même dans la chaîne morte. **Un appelant qui existe
   n'est pas un appelant joignable.**

## Trouvé en rangeant — `REQ-AXO-902637`

Cinq compteurs `ingress_subtree_hint*` sont **lus par trois surfaces** (status MCP, dashboard
LiveView, `qualify_ingestion_run.py`) et **écrits par aucune**, tous avec un défaut `0`. Trois
surfaces affirment mesurer un flux dont le producteur a été arraché. Non avalé dans cette tranche :
périmètre Rust + Elixir + Python.

## Vérifié pour ne pas élargir

`should_descend_into_directory` reste sans appelant de production — mais il l'était **déjà**. Ses
deux exclusions propres restent joignables (`explain_ignore_decision`, `should_process_path`) et
**0 fichier n'est indexé sous `.worktrees`**. Pas de septième instance : mesuré, pas présumé.

## Reste opérateur

1. **Le promote** — 4 commits non servis, `phase: clean` 7/7, coupure mesurée 6,1 s (s143). Le
   verdict se lit sur `status mode=brief`, jamais sur le journal du promote (`REQ-AXO-902628`).
2. **`REQ-AXO-902638`** — les trois segments, 618 fichiers d'un locataire tiers, réversible.

Corps complet : `CPT-AXO-052`, deux dernières sections.
