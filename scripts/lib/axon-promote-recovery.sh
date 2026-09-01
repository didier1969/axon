#!/usr/bin/env bash
# RÉUTILISE : néant — vérifié via `axon query "classification des gates de promote
# par palier de reprise, appartenance ensembliste sur failed_gates"` (aucun symbole
# couvrant). `failure_classification_tests` (tools_friction.rs) classe des frictions
# MCP, pas des gates de release ; `axon-supervisor.sh` AGIT sur les rôles au lieu de
# décider quel palier appliquer.
#
# REQ-AXO-902590 — quel palier de reprise appliquer, à partir des gates rouges.
#
# LE DÉFAUT QUE CE FICHIER FERME. `promote_live_safe.sh` décidait par ÉGALITÉ DE
# CHAÎNE sur la liste jointe :
#
#     if [[ "$recon_failed" == "indexer_alive" ]]; then          # tier 1
#     if [[ ... && "$recon_failed" != "indexer_alive" ]]; then    # tier 2, coupe le brain
#
# Deux gates rouges ensemble donnent `indexer_alive,autre`, l'égalité tombe, et le
# promote va au tier 2 : `stop --hard` + redémarrage complet, « THIS INTERRUPTS THE
# BRAIN » — 3 min 53 d'indisponibilité mesurées au promote 1399, assez pour que des
# clients tiers déclarent un crash et redémarrent le serveur eux-mêmes.
#
# Le pire moment possible : la co-occurrence arrive quand le runtime est DÉJÀ dégradé.
#
# POURQUOI TROIS SEAUX ET NON DEUX. Un gate rouge ne dit pas forcément qu'un
# redémarrage réparerait quelque chose. `last_promote_attempt` (REQ-AXO-902585) est un
# fait HISTORIQUE : « la dernière tentative enregistrée a échoué ». Il reste rouge
# jusqu'au prochain promote réussi, et AUCUN redémarrage ne le change. Le compter dans
# la décision ferait basculer en tier 2 toute panne d'indexeur survenant après un
# promote raté — c'est-à-dire un cas ordinaire, quatre fois le 2026-09-01.
#
# INCONNU ⇒ TIER 2, délibérément. Un nom hors des deux listes n'élargit pas le filet :
# on ne sait pas ce qu'il décrit, donc on garde la reprise la plus complète. Le test
# Rust `promote_status_n_emet_que_des_gates_classes_cote_shell` rougit si un gate neuf
# apparaît sans être classé ici, pour que ce choix reste un choix et non un oubli.

# Gates qui décrivent la disponibilité COURANTE du runtime et qu'un redémarrage du
# SEUL indexeur peut réparer. Le brain continue de servir pendant ce palier.
#
# ⚠ `indexer_process_stable` en `Fail` signifie « boucle de redémarrage détectée », et
# le tier 1 y répond par un redémarrage de plus. `axon_restart_role_verified` échouera
# alors (pas de pid stable) et l'on tombera en tier 2 : pas de dégât, mais ~180 s
# perdues avant l'escalade. Non traité ici — ce fichier décide du palier, il ne réécrit
# pas l'échelle.
AXON_PROMOTE_INDEXER_ONLY_GATES=(indexer_alive indexer_process_stable)

# Gates qui décrivent la disponibilité courante mais qu'un redémarrage du SEUL
# indexeur ne peut PAS réparer : il faut la reprise complète. Le comportement est le
# même qu'un nom inconnu ; la différence est qu'ici c'est un CHOIX écrit, vérifiable
# par le test Rust, et non une ignorance qui tombe par défaut au bon endroit.
AXON_PROMOTE_FULL_RESTART_GATES=(brain_serving)

# Gates qui ne décrivent PAS la disponibilité courante : aucun redémarrage ne les
# répare, donc ils ne participent à AUCUNE décision de reprise. Ils restent rendus
# dans le journal — les écarter d'une DÉCISION n'est pas les rendre invisibles.
AXON_PROMOTE_NON_RUNTIME_GATES=(
  last_promote_attempt   # REQ-AXO-902585 — fait historique, pas un état courant
  qualification_passed   # verdict d'une qualification passée, idem
  manifest_runtime_match # dérive manifeste↔runtime : un redémarrage ne la corrige pas
  no_stale_pending       # staging orphelin : se résout par un cutover, pas par un stop
)

# Vrai si $1 figure dans le tableau nommé $2.
_axon_promote_gate_in() {
  local needle="$1" arr="$2[@]" candidate
  for candidate in "${!arr}"; do
    [[ "$candidate" == "$needle" ]] && return 0
  done
  return 1
}

# Découpe la liste jointe (`a,b,c`) en gates, en ignorant les entrées vides.
_axon_promote_split_gates() {
  local joined="${1:-}" IFS=','
  read -r -a _AXON_PROMOTE_GATES <<< "$joined"
}

# Rend, sur stdout, les gates RETENUS pour la décision de reprise (séparés par des
# virgules) : ceux qui décrivent la disponibilité courante.
axon_promote_retained_gates() {
  local gate out=()
  _axon_promote_split_gates "${1:-}"
  for gate in "${_AXON_PROMOTE_GATES[@]:-}"; do
    [[ -z "$gate" ]] && continue
    _axon_promote_gate_in "$gate" AXON_PROMOTE_NON_RUNTIME_GATES && continue
    out+=("$gate")
  done
  local IFS=','; printf '%s' "${out[*]:-}"
}

# Rend les gates ÉCARTÉS de la décision. Le journal doit dire ce qui a été retenu ET
# ce qui a été écarté : sinon un opérateur lit « failed_gates: a,b — TIER-1 » et doit
# deviner pourquoi `b` n'a pas compté. Un journal qui tait sa propre règle est la
# classe de défaut que MIL-AXO-054 poursuit.
axon_promote_ignored_gates() {
  local gate out=()
  _axon_promote_split_gates "${1:-}"
  for gate in "${_AXON_PROMOTE_GATES[@]:-}"; do
    [[ -z "$gate" ]] && continue
    _axon_promote_gate_in "$gate" AXON_PROMOTE_NON_RUNTIME_GATES && out+=("$gate")
  done
  local IFS=','; printf '%s' "${out[*]:-}"
}

# TIER 1 — vrai quand il y a au moins un gate retenu et que TOUS sont réparables par
# le seul indexeur. Le « au moins un » compte : une liste sans aucun gate retenu ne
# justifie aucun redémarrage.
axon_promote_is_indexer_only_failure() {
  local gate seen=0
  _axon_promote_split_gates "$(axon_promote_retained_gates "${1:-}")"
  for gate in "${_AXON_PROMOTE_GATES[@]:-}"; do
    [[ -z "$gate" ]] && continue
    seen=1
    _axon_promote_gate_in "$gate" AXON_PROMOTE_INDEXER_ONLY_GATES || return 1
  done
  [[ "$seen" -eq 1 ]]
}

# TIER 2 — vrai quand au moins un gate retenu N'EST PAS réparable par le seul
# indexeur. Un nom inconnu tombe ici : on ne sait pas ce qu'il décrit, on garde la
# reprise complète plutôt que de supposer qu'elle est inutile.
#
# Conséquence NEUVE, et voulue : une liste ne portant QUE des gates hors disponibilité
# courante (`last_promote_attempt` seul) ne déclenche NI tier 1 NI tier 2. Le promote
# échoue alors fermé, sans couper le service — car il n'y a rien à redémarrer.
axon_promote_needs_full_restart() {
  local gate
  _axon_promote_split_gates "$(axon_promote_retained_gates "${1:-}")"
  for gate in "${_AXON_PROMOTE_GATES[@]:-}"; do
    [[ -z "$gate" ]] && continue
    _axon_promote_gate_in "$gate" AXON_PROMOTE_INDEXER_ONLY_GATES || return 0
  done
  return 1
}
