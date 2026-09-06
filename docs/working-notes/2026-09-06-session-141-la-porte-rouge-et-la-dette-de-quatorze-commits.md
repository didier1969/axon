# Session 141 — la porte rouge, et la dette de vingt-et-un commits

2026-09-06 · HEAD `3108c8f0` · servi `v0.8.0-1727-g8ade669a` · **promote interdit**

## Ce qui a été livré

Quatre lots du plan approuvé, commités et poussés, chacun avec ses tests ciblés verts.

| commit | ce que ça corrige |
|---|---|
| `da85dae5` | le gardien d'égalité exacte des dispositions, rouge depuis `5c7218cd` |
| `be588c16` | un *lost update* dans `IstSnapshotCache` — six lignes, `ArcSwap::rcu` |
| `fc8a7c5f` | le matching d'exemption `wiring`, de la feuille au suffixe qualifié |
| `3108c8f0` | le bundle d'ouverture, servi en identité plutôt qu'en corps entiers |

Deux méritent un mot.

**`be588c16`** est le plus petit et le plus intéressant. Le REQ annonçait une passe large sur ~20
sites d'appel à sérialiser ; c'était le symptôme. `publish` et `evict` faisaient un
read-modify-write non atomique sur la carte entière, si bien que deux écrivains s'effaçaient l'un
l'autre **même sur des projets disjoints** — le `store` final remplaçait tout. Le partenaire de
course n'était pas entre les tests RAM : c'est `ensure_ram_snapshot_warm`, que déclenchent les
treize tests construisant un serveur. Et c'est aussi une course de production, qu'un verrou de test
n'aurait pas vue. Vingt tours à 35/35 : le rouge aléatoire qui attendait à la porte est mort.

**`3108c8f0`** n'atteint pas son critère et le dit. Les six coupes retirent ~85 000 caractères ; il
en reste ~18-20 000 contre les 12 000 visés. Le poste dominant restant est `kickoff_prompt`, que le
REQ classe lui-même parmi les intouchables. C'est un arbitrage opérateur, pas un oubli.

## Et puis la porte

Rejouée en entier après ces quatre lots : **exit 12, étape 2 sur 5**. `cargo test --lib` rend
**2 216 passed / 8 failed**.

La dernière porte verte connue est celle de la session 139, sur `8ade669a` : 2 162 / 0. Entre les
deux, vingt-et-un commits — les dix du plan token, les quatre d'aujourd'hui, et le reste — dont
**aucun n'a vu une porte complète**. La consigne des tranches précédentes était « pas de test full »,
et elle était raisonnable prise une par une.

Prise plusieurs fois de suite, elle a produit exactement ce qu'on pouvait craindre : quand le rouge
apparaît enfin, il faut rouvrir vingt-et-un diffs pour savoir lequel accuser.

### Ce qui est prouvé

Trois des huit sont cassés par `43758dc8` — ma tranche d'hier. `REQ-AXO-902596` a fait de
`token_budget` une borne dure qui **vide** des bandes du paquet, dont `structural_neighbors`. Or
`context_and_analysis.rs:3220` appelle `retrieve_context` avec `token_budget: 1200` et exige
`structural_neighbors` non vide.

Ce n'est pas un test fragile qu'on ajuste. C'est un contrat client que la coupe a changé en silence,
et le test faisait son travail en le disant.

### Ce qui reste à instruire

Cinq échecs sans cause établie : `sql` et son miroir `structuredContent.rendered_text` ; le contrat
`REQ-AXO-902409` — « N éléments stockés, N rendus dans le texte » — violé par un outil non
identifié ; trois `axon_commit_work` qui refusent sur `GUI-AXO-001`, dont un seul vise le wiring,
ce qui n'explique pas les deux autres.

## Deux erreurs de méthode, et ce qu'elles ont appris

**J'ai conclu trop vite à une cause d'environnement.** Le log est truffé de
`Failed to load ONNX Runtime dylib: dlopen failed` — huit panics en gras — et j'y ai vu
l'explication des huit échecs. J'ai reconstruit `LD_LIBRARY_PATH`, rejoué la porte, et obtenu
**exactement les mêmes huit**. Ces panics sont du bruit permanent que les tests concernés absorbent.

La leçon n'est pas « ne pas se tromper » mais « le distinguer coûte un test » : corriger la
condition supposée et rejouer. Si le verdict ne bouge pas d'un iota, le message était du bruit. Le
piège tient à ce que le bruit est spectaculaire et la vraie cause discrète — huit panics contre une
assertion sur un champ absent d'un objet JSON. La discipline qui coupe court : lire l'assertion
exacte de chaque test tombé avant de chercher un dénominateur commun dans ce qui défile.

**Le trou d'environnement était pourtant réel**, et c'est une seconde vérité sans lien avec la
première. `devenv.nix` refuse délibérément d'ajouter ORT (`REQ-AXO-901630`), `axon-ort-runtime.sh`
n'est sourcé qu'au démarrage du runtime, et le courtier strippe 81 variables. La porte ne chargeait
ORT que par héritage du shell appelant — donc par accident, selon ce que le terminal avait fait
avant. `GUI-AXO-1034` porte maintenant le préambule qui compose ce chemin, et deux autres pièges
mesurés le même jour : sans redirection vers un fichier, un job du courtier ne conserve pas sa
sortie et un `exit_code` ne dit que l'étape ; et `nexus-job submit` rend `rc=0` sur une soumission
encore en file, ce qui laisse déclarer verte une porte qui n'a pas commencé.

## L'état, sans arrondi

Le promote est interdit et doit le rester : servir un binaire dont la porte est rouge diffuserait un
contrat cassé sur `retrieve_context` à soixante-quinze locataires.

`REQ-AXO-902402` est ouvert dans un état inconfortable. Les sept lignes `IndexedFile` ont été
supprimées — voie A, autorisée — mais la reconstruction est freinée par la pression mémoire (swap
plein, indexeur en `recovery_override`). Les sept morceaux étaient tronqués ; ils sont maintenant
absents. C'est temporairement pire qu'avant, réversible par construction, et pas encore rétabli. La
vérification après cutover se fait en deux volets, parce que le compte à zéro vaut déjà zéro
aujourd'hui — par soustraction.
