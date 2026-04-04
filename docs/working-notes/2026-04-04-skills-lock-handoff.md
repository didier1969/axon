---
title: Axon Skills Lock Handoff
date: 2026-04-04
branch: fix/sunburst-visibility
status: skills-source-realigned
---

# Scope

Ce handoff capture l'etat reel du verrouillage des skills Axon entre le repo et les registries operateurs locaux.

# Reality

- La source de verite active pour les skills reste `/home/dstadel/.claude/skills`.
- Codex resolve les skills via `/home/dstadel/.codex/skills`, qui pointe deja sur `~/.claude/skills`.
- Gemini resolve aussi ses skills via `~/.claude/skills`.
- Le repo Axon porte une copie versionnee de reference sous `docs/skills/`.

# Verified On 2026-04-04

- `readlink -f /home/dstadel/.codex/skills/axon-soll-operator` -> `/home/dstadel/.claude/skills/axon-soll-operator`
- `test -f /home/dstadel/.codex/skills/axon-soll-operator/SKILL.md` -> OK
- `readlink -f /home/dstadel/.gemini/skills` -> `/home/dstadel/.claude/skills`
- `grep -n "Axon Skills Resolution Policy" /home/dstadel/.gemini/GEMINI.md` -> section presente

# Dominant Finding

Le verrouillage n'etait pas complet:

- `docs/skills/axon-soll-operator/SKILL.md` avait avance
- `~/.claude/skills/axon-soll-operator/SKILL.md` etait en retard
- donc la copie versionnee du repo et la source de verite operateur divergeaient

# Remediation Applied

- la version repo `docs/skills/axon-soll-operator/SKILL.md` a ete recopied vers `/home/dstadel/.claude/skills/axon-soll-operator/SKILL.md`
- le bridge Codex n'a pas eu besoin d'etre modifie
- la politique Gemini n'a pas eu besoin d'etre modifiee

# Current Rule

Pour Axon:

- editer d'abord la copie versionnee dans le repo
- synchroniser ensuite `~/.claude/skills/<skill>`
- compter sur le bridge `~/.codex/skills -> ~/.claude/skills`
- redemarrer les nouvelles sessions Codex/Gemini apres ajout ou changement de skill

# Next Logical Step

- si d'autres skills Axon locaux sont ajoutes, appliquer le meme schema sans dupliquer une source parallele
- si la surface SOLL MCP ou CLI change encore, mettre a jour `docs/skills/axon-soll-operator/SKILL.md` dans la meme vague
