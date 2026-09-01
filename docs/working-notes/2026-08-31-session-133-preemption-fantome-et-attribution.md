# Session 133 — la préemption fantôme : comment on corrige trois causes de mort d'un processus qui ne meurt pas

**2026-08-31** · audit-only, append-only · état vivant : `CPT-AXO-052` (dernière section)

## Ce que la session croyait à l'ouverture

L'indexeur était « mort » depuis 16 h 40. La session 132 avait diagnostiqué et corrigé **trois causes de mort** (init CUDA, course DDL, sonde de vivacité), puis identifié un quatrième arrêt comme une préemption par le courtier VPC. Le pointeur affirmait : « le courtier pose le frein et échoue à l'exécuter — c'est un frein bloqué ».

Rien de tout cela n'était exact.

## Les quatre révisions, dans l'ordre où la mesure les a imposées

| # | Ce qui était cru | Ce que la mesure a établi |
|---|---|---|
| 1 | « Frein bloqué que personne ne peut remettre » | Le courtier tourne **toutes les 2 s** et repose le marqueur en quelques secondes. Il oscille : deux `paused_at` à 375 s d'écart |
| 2 | « Le swap saturé explique le PSI » | Vrai mais sans valeur : VPC l'avait écarté **la veille**, docstring à l'appui — le swap mesure l'histoire, pas la pression |
| 3 | « La contention est fabriquée par le confinement : Axon se préempte par sa propre lecture » | **FAUX.** `axon-live.service` : `high=0`, `max=0`, `oom_kill=0`, PSI 0,00. Nous ne produisions rien |
| 4 | « L'indexeur est mort » | **Il finissait ses passes.** `process get` → `Completed`, `exit_code: 0` |

## La cause réelle, trouvée par VPC

`daggy-typedb.service` (projet DGD) partageait `nexus-core.slice`. Un `MemoryHigh` abaissé de 5G à 3G sur un service qui consommait 3,3G → reclaim permanent → **79 056 050 événements `high`, 82,2 % de toute la pression de la tranche**. Le courtier lisait le PSI de la tranche sans vérifier qui le produisait.

Nous avons attendu 20 h à cause du résidu d'un tiers, en affichant zéro contribution.

## Les deux erreurs de raisonnement, nommées pour ne pas les refaire

**Corréler n'est pas imputer.** « La tranche souffre » + « j'y suis » ne donne pas « je souffre », encore moins « je cause ». Le PSI n'est pas additif entre cgroups enfants — l'imputation exige un compteur local (`memory.events`.`high`), et c'est exactement ce que VPC a livré. Notre explication était mécaniquement plausible, élégante, et fausse. Elle a été envoyée avec un ton d'exigence avant d'être vérifiable.

**Un instantané n'est pas un régime.** À 20h20 la machine était à 85,6 % idle ; à 20h40, load 47,8 avec 18 rustc. Deux relevés à vingt minutes d'écart, deux diagnostics opposés. Avant d'accuser un mécanisme, il faut remonter l'arbre des processus jusqu'à la racine pour savoir à qui appartient la charge quand elle revient.

Ce qui a sauvé la session : avoir retiré l'explication n° 3 **de nous-mêmes**, en la qualifiant de « plausible, non établie », avant que VPC ne la réfute. Une explication qu'aucune mesure n'isole se retire, elle ne se défend pas.

## Le défaut que ça révèle chez nous — `REQ-AXO-902581` (P0)

`crashed_or_abandoned` est **inféré** d'un battement PG périmé. Sur un rôle qui travaille par passes et se termine proprement entre deux, l'inférence est structurellement fausse — et elle est rendue comme une observation.

Coût : deux sessions. La 132 a « corrigé trois causes de mort » et pris le déplacement du point d'arrêt de 15 s à 7 min 26 pour une preuve de succès. La 133 a bâti un dossier d'interblocage sur la même prémisse. **Un verdict de panne inventé oriente des sessions entières vers des causes qui n'existent pas.**

## La classe, et ses quatre instances mesurées le même jour

`REQ-AXO-902409` — « une surface n'affirme jamais plus qu'elle ne sait ».

| instance | forme |
|---|---|
| Les **trois** gates SOLL de `axon_handoff_check` | `LIMIT 50` rendu comme un compte : annonce 50, le réel est 73 |
| `soll_get` `sections`/`section` | tronque `content[0].text`, jamais `data.description` — 134 369 caractères pour ~120 jetons voulus |
| `sql` | enveloppe `ok` sans lignes ni colonnes sur une agrégation valide (3ᵉ reproduction) |
| `crashed_or_abandoned` | excès **modal** : une inférence rendue comme une observation |

Les trois premières sont quantitatives, la quatrième modale. Même racine : une surface qui ne sépare pas ce qu'elle sait de ce qu'elle rend.

## Ce qui reste bloqué, et pourquoi ce n'est pas notre défaut

Le courtier refuse toute compilation d'Axon : `cargo` et `devenv` sont interceptés par un wrapper de PATH qui les soumet à l'admission, et la file est saturée (LLL compile en boucle, TRADER_ELIXIR_V2 enchaîne sa CI). Un `cargo test` sur **un seul test nommé** est classé `huge` à 12 Gio d'estimation forfaitaire — une estimation juste passerait où le forfait bloque.

Conséquence : le test de `REQ-AXO-902580` n'a jamais compilé, et le promote a échoué au step 1 après avoir franchi les deux gates précédents.

## Ordre de l'opérateur, exécuté

« Aucun résidu, immédiatement. » Nous en avions : **5,36 Gio** en 7 worktrees de promote orphelins des 28-29/08, retirés par `git worktree remove` + `prune` (ils étaient enregistrés — un `rm -rf` aurait laissé le registre menteur).

La règle qui compte n'est pas le ménage, c'est sa **place** : le nettoyage se pose à la création de la ressource (`trap`, `Drop`, `TempDir`), jamais en fin de script. Le résidu naît du crash au milieu, donc c'est le seul cas qui compte. Un banc de test doit être inoffensif **par défaut**, pas par la vigilance de celui qui ajoute un cas — VPC l'a appris en écrivant dans notre vrai marqueur de pause depuis ses tests.

## Pièges d'outillage récoltés

- `systemctl show -p Slice` rend la valeur **configurée**, pas la tranche effective → `-p ControlGroup`.
- Un drop-in systemd s'applique à moitié : `MemoryHigh`/`CPUQuota` au `daemon-reload`, `Slice=` exige un redémarrage.
- `/tmp` est un **tmpfs** : ce qu'on y laisse est de la RAM, invisible à `ps`, et ampute `MemAvailable`.
- Journaliser `stderr[-500:]` garde la queue et jette le message d'erreur, qui vient en tête.


---

# Suite — la nuit du 31/08 au 01/09 : livrer quand la porte de compilation est fermée

## Ce que la session a livré

`80e71bd7` — l'init GPU nomme désormais l'étape où elle meurt. Cause 1 de `REQ-AXO-902576`.

Le point de conception mérite d'être retenu : **un `tracing::info!` ne suffit pas** quand le processus est abattu par un signal, parce que le buffer du souscripteur peut n'être jamais vidé. Le log meurt avec le processus qu'il devait décrire. D'où un témoin écrit par `fs::write` **synchrone** avant chaque étape native — il survit au SIGSEGV.

Prouvé dans les deux sens : job `1788215582` vert, job `1788211136` rouge après mutation, sur l'assertion exacte. Un garde incapable de rendre l'autre verdict ne prouve rien.

## Le vrai adversaire de la nuit n'était pas le code

Six jobs de compilation morts en file. La cause a mis quatre hypothèses à mourir avant d'être trouvée :

| hypothèse | verdict |
|---|---|
| « le courtier refuse par contention » | faux — refus persistant sur machine à 0 rustc |
| « le p95 est empoisonné par les forfaits du wrapper » | **faux** — il apprend des pics réels ; retiré |
| « 12 G est un forfait arbitraire de VPC » | **faux** — c'est notre propre norme, `GUI-AXO-1034` ; retiré |
| « le p95 est un max » | **VRAI**, et c'est une ligne |

`peaks[min(len-1, int(len*0.95))]` avec une fenêtre `LIMIT 20` rend le dernier index pour tout n. Sur nos 19 mesures : médiane **3,74 G**, valeur retenue **12,00 G**. Une seule porte de build complète dans la fenêtre fait réserver 12 G aux vingt jobs suivants — `cargo --version` compris.

**Deux griefs retirés pour un trouvé.** C'est le bon ratio quand on cherche vraiment.

## Deux fautes de méthode, à ne pas refaire

1. **`cargo … | tail -12` masque le code retour.** Le job a rapporté `succeeded exit=0` alors que `test --lib` était `FAILED`. C'est exactement la classe `REQ-AXO-902409` que cette session documente depuis des heures — introduite par moi, dans ma propre commande. Une surface qui ment n'a pas besoin d'être écrite par quelqu'un d'autre.
2. **Division par 10⁹ au lieu de 2³⁰.** « Il faut 24 GiB » portait sur 23,27. Trois messages bâtis sur un chiffre faux avant de le vérifier.

## Ce qui a été touché hors de notre territoire, et rendu

Un override `NEXUS_ADMISSION_RESERVE=2G` sur le courtier du parc — service de VPC, pas le nôtre. Posé sur directive opérateur à 00h32, documenté dans le fichier lui-même (pourquoi, mesure justificative, effet de bord assumé, commande de retour), **retiré dès la livraison passée**. Aucun drop-in ne subsiste.

La règle appliquée : on peut emprunter le levier d'un voisin si on le lui dit, si on écrit pourquoi, et si on le rend. Pas autrement.

## Ce que les voisins ont apporté, et qui vaut plus que nos mesures

**VPC** : la cause réelle des 20 h de préemption — `daggy-typedb` avec un `MemoryHigh` sous sa consommation, 82,2 % de la pression de la tranche, pendant que `axon-live` mesurait PSI 0,00. Et surtout ce piège de mesure, qui dépasse l'incident : *un plafond trop bas fait paraître son prisonnier plus gros qu'il n'est* — `opv-serve` lisait 8,0 Go sous un high de 8,0, desserré il en pèse 6,1. On entre dans cette boucle de bonne foi en regardant `top`.

**NEX** : `GUI-PRO-120` disait littéralement l'inverse d'elle-même. Titre, énoncé et exemple d'un côté ; ligne RÉPARATION de l'autre — la dernière lue, la seule à l'impératif. Aucun outil ne l'avait vue ; il a fallu un lecteur qui compare la consigne à la règle avant d'exécuter douze écritures.

**LLL** : 2,2 Gio de RAM rendus en dix minutes, et une remarque de forme qu'il faut garder honnêtement — notre message leur a été actionnable non par courtoisie, mais parce que nous avions la chaîne causale au moment de l'écrire. Quand on ne l'a pas, l'exigence remplit le vide et vise à côté. C'est ce qui était arrivé une heure plus tôt avec VPC.
