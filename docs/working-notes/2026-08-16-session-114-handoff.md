# Session 114 — handoff (2026-08-16)

Canonique : `CPT-AXO-052` (session_pointer) + MEMORY.md. Ce note = récit.

## ⚠️ Fait dominant : la machine migre WSL → NixOS

L'opérateur a décidé de quitter WSL2 pour un **NixOS natif en dual-boot**. Déclencheur
concret survenu CETTE session : en redémarrant l'indexeur live (prépa promote, ~09:00),
sa rafale GPU a **wedgé le canal WSL2 unique `dxgvmb`** (~09:19) et **tué d'un coup ses 25
sessions agent-deck** (toutes `use_chrome=true` → Chrome partage le même canal GPU physique).
agent-deck (sans GPU) a survécu ; ses enfants Chrome sont morts ensemble. RCA confirmée en
live : indexer tokio-worker + `nvidia-smi` en D-state sur `dxgvmb_send_sync_msg`.

Diagnostic élargi : WSL2 = canal GPU sérialisé + RAM plafonnée (47/63 Go) + fs 9p lent.
NixOS natif supprime la classe (GPU direct, AMD iGPU pour l'écran + RTX 3070 pour le calcul
Axon, rollback atomique du driver). Matériel : OMEN 16, Ryzen 5800H, 63 Go, RTX 3070 Laptop,
2 NVMe (WDC 512 Windows / Kingston 2 To = D: MBR où vit le vhdx WSL 1,2 To).

### Kit de migration (D:\OneDrive\nixos-migration-backup\20260816-1114\, cloud-backed)
- Procédure : `RUNBOOK.md`. Prep Windows : `prep-windows.ps1` (hibernation off + compact vhdx).
- NixOS : `configuration.nix` (GNOME + GPU hybride + WhatsApp + claude-code), `gpu-busids.sh`
  (auto-détecte/écrit les bus IDs), `restore.sh` (remonte tout).
- Sauvegarde 436 Mo : SOLL dump (re-dumpé en fin de handoff), home-config (SSH+.claude+agent-deck),
  secrets, 43 snapshots WIP git, 7 bundles git (repos locaux sans remote — trou trouvé et bouché
  à la revérif : 8 repos sans remote, dont `infra` à 0 commit = tout en untracked déjà sauvé).
- ISO NixOS 26.05 graphical téléchargée (sha256 validé après 2 troncatures dues à la connexion)
  + gravée sur clé USB 8 Go via Rufus.

## Livré côté Axon (2 commits poussés, HEAD 5ad5bf3d)

- **`393229a2` REQ-AXO-902339** — le DDL réclamait son verrou avant de savoir s'il avait
  quelque chose à faire. Sonde jetable (rejeu du DDL derrière un écrivain) → **28 énoncés**
  bloquants dans 3 fichiers, 5 formes (pas « 36 ADD COLUMN »). 4 gardes catalogue
  (`add_column_if_absent` / `set_column_default_if_absent` / `create_index_if_absent` /
  `create_trigger_if_absent`) dans `00_extensions.sql`, 28 sites réécrits, sonde 28→0. Test
  `ddl_lock_tests.rs` avec **contrôle négatif** (qui a falsifié ma propre affirmation : `DROP
  TRIGGER IF EXISTS` sur un trigger absent ne verrouille pas). **Reste `current`** : la preuve
  est un promote réel (step 5b), pas la suite. Vérifié en lecture live : le prochain bootstrap
  prendra 0 verrou sur `axon_live` (27 index + 6 triggers + les 8 statuts déjà présents).
- **`5ad5bf3d` REQ-AXO-902338** (delivered) — un drapeau inconnu démarrait le rôle. `role_cli.rs` :
  `--version`/`--help` sortent 0, argument inconnu refusé (sortie 2), sur les **3** binaires
  (axon-brain, axon-indexer, axon-core). La 3e défense annoncée (verrou d'écrivain) existait
  déjà et fonctionne ; le vrai résidu (refus APRÈS écriture d'état partagé) → **REQ-AXO-902341**.

Porte complète verte (--lib 1807/0/738s, --bins, build --tests), dans devenv shell, indexeur
arrêté puis relancé.

## Ce qui reste (post-migration)

1. **Promote** → prouve le step 5b de 902339 → le passer `delivered` depuis le journal du
   promote (PAS depuis la suite). AVANT : basculer la règle mémoire « ne pas exécuter un binaire
   de rôle » (les binaires installés restent pré-correctif jusqu'au cutover).
2. **REQ-902340** (P0) — 161 fichiers hors fenêtre, purge = +26 % index → arbitrage opérateur.
3. **REQ-902341** (P2) — verrou d'écrivain avant écritures d'état partagé au boot.
4. **Mailbox** : FSF veut purger le doublon `ELE` (2 nœuds placeholder, sûr — scoper au code ELE
   pas au path FSF) ; VPC question soll_export git vs soll.Revision (non urgent).

## Leçons (practices 961/962/963/991/993)
Périmètre d'une classe = mesuré par sonde · le verrou PG est pris avant `IF NOT EXISTS` ·
`cargo test --bins` ne construit pas les binaires · complétude d'un download = taille+sha256 ·
redémarrer l'indexeur n'est pas GPU-isolé des voisins.
