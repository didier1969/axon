# Eval-matrix inputs — REQ-AXO-91586 / Path A δ protocol

72 prompt templates pour la matrice d'évaluation methodology-vs-baseline
(8 SKI cases × 3 conditions × 3 runs).

## Protocole opérateur

Tu ouvres **3 sessions Claude Code séparées**, profils isolés :

| Condition | Setup |
|---|---|
| `bare` | Claude Code sans skills, sans MCP (control) |
| `axon` | Claude Code avec MCP Axon enabled (subject) |
| `sota` | Claude Code avec un SKILL.md filesystem chargé, sans Axon MCP (baseline industrie) |

## Boucle

Pour chaque fichier `prompts/{ski}__{cond}__{run}.prompt.txt` :

1. Tu ouvres la session de la condition correspondante
2. Tu pastes le contenu intégral du `.prompt.txt` comme nouveau user message
3. Tu copies la réponse complète de Claude (jusqu'au marker `__END__` qu'il aura inclus)
4. Tu sauvegardes la réponse sous `{ski}__{cond}__{run}.txt` (sans le préfixe `prompt.`) **dans CE répertoire**, pas dans `prompts/`

Exemple : `prompts/SKI-PRO-999__axon__1.prompt.txt` → `SKI-PRO-999__axon__1.txt`

## Scoring

Quand toutes les réponses sont collectées (les 72) :

```
python tools/eval-matrix/run.py --batch-input tools/eval-matrix/inputs --output tools/eval-matrix/inputs/results
```

Le harness scorera chaque réponse (contract + rubric) et émettra un `.json`
par réponse sous `results/`.

## Cadence estimée

~5 min par paste/copy/save = 72 × 5 = **6h** total étalées sur 2-3 sessions
opérateur. MVP statistiquement défendable (72 datapoints vs REQ-91586
demande 50 tâches × 3 conditions = 150 — on couvre 8 cases canoniques
× 3 runs pour avoir le signal de variance, et on documente l'écart au
spec dans le writeup `docs/research/eval-matrix-2026-05.md`).

## Anti-position-bias

Pour réduire l'effet d'ordre, mélange l'ordre des prompts (e.g., shuffle
les noms de fichiers à l'aveugle avant de paste-loop). Le scoring final
ne dépend pas de l'ordre de collection.

## Confidentialité

Aucune des 8 SKI cases ne contient de données sensibles ; tu peux paster
en toute sécurité dans n'importe quelle session Claude tant que tu
n'attaches pas de fichiers projet.
