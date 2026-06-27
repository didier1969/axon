# Session 92 handoff — 2026-06-27

Audit-only prose. Canonique = SOLL `CPT-AXO-052` (session_pointer) + git log. Ne remplace ni la SOLL ni le pointer.

## Thème
Session marathon multi-agent : épuisement du backlog autonome AXO + livraison des 2 chantiers phares cross-projet (mailbox + mémoire gouvernée), puis déblocage des 2 forks operator-gated (ContractNode, MBX-5).

## Livré (tout LIVE @ v0.8.0-1262-gdde6f323)
- **Mailbox MBX-1→11** : MVP + Agent Cards (A2A) + pub/sub/rooms/broadcast + leases + render/tap + conformance harness + MBX-5 mécanisme (secret per-projet + scaffold ACL, HMAC gardé). MIL-046/047/048 + umbrella 902112 fermés.
- **Mémoire gouvernée auto-améliorante (REQ-902131)** : practice_put (write-gate anti-poison via contradiction_check) / recall (ANN scopé + re-rank trust×retrievability) / tick (FSRS decay + prune) / card. + advisory mode-échec (902132, découvert au E2E live). Bug vector_literal double-quote attrapé par le dev-first.
- **ContractNode 6/6 (REQ-902087)** : décision opérateur = modèle A (tables soll.* dédiées). S1 persistance + sceau Merkle + S6 réconciliation IST↔contrat (drift typé). Red-flag `realizes:`→MDA non déclenché.
- **Reconciler control-plane COMPLET (REQ-902111)** : T1 + liveness (dogfoodé live) + stop-FSM (gates purs + câblage axonctl) + post-check step 6c + Ascent/Datalog T2 (release+liveness gates+phase precedence, 3 tests différentiels exhaustifs vs oracle Rust). MIL-044/045 fermés.
- **HNSW corruption corpus-wide RÉSOLUE** : RCA sous-agent (réfute mon hypothèse over-filtering) → vraie cause = 62k chunks identiques (vieux chunker fan-out doc-entier, embedder tronque 512 tokens → embeddings identiques → îles HNSW). Purge des 11 fichiers cap-violating + REINDEX → reachability rétablie (iterative_scan LIMIT 5000 → 5000), recall corpus-wide restauré.
- **NLI** net-margin + exact-scan. **GPU** NVML migration (nvidia-smi→ctypes) + auto-release lane B/C. **Promote durci ×8** (in-place restart, DDL step 5b, create_manifest, auto-resume, rollback build-info, step 6c reconcile).

## Méthode
~20 sous-agents orchestrés en worktrees isolés (recherche/RCA/design + implémentation Python/Rust edit-only), intégrés en série côté build (lock cargo orchestrateur). 2 incidents promote récupérés (verrou sous-agent DB pendant DDL apply ; arbre sale pendant preflight) — leçons : pas de requête DB lourde ni d'édition pendant un promote.

## Incidents / leçons
- Promote bloqué par AccessShareLock d'un sous-agent dedup → CREATE INDEX du bootstrap timeout → live down → récupéré via cancel + resume.
- Promote preflight échoué sur arbre sale (édition 902079 non-committée pendant le promote).
- vector_literal retourne déjà '[...]' quoté → ne pas re-quoter (bug attrapé dev-first).

## Reste (opérateur)
- MBX-5 POLICY (chiffrement/H1/JWS/ACL-default) gated. ContractNode S7/S8 (902094/095, sous MIL-041). MIL-043 package client deferred. Dette : ~70 vieux REQ sans Traceability (sessions antérieures).
