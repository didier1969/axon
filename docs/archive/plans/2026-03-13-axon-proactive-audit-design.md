# Axon - Proactive Audit Design

## Context
Nous devons implémenter une "Alerte automatique dès qu'un changement dégrade le score de sécurité". Pour respecter la séparation des plans (Control Plane = Elixir, Data Plane = Rust), cette logique d'état et d'orchestration sera portée par Elixir.

## Objectif
Permettre au système d'informer l'utilisateur (via le dashboard LiveView) instantanément lorsqu'une modification de code (Hot Path) a réduit le score de sécurité calculé par le moteur d'Audit OWASP de Rust.

## Architecture & Approche (Control Plane Detection)

### 1. Stockage de l'État dans OTP (Elixir)
- Le processus gérant la réception des événements TCP (soit un client Bridge dédié, soit le composant en charge de `BridgeEvent.FileIndexed`) conservera un dictionnaire des scores de sécurité connus par projet.

### 2. Détection de la Dégradation
- Lors de la réception de l'événement `BridgeEvent.FileIndexed` venant de la socket TCP Rust, Elixir extrait le `security_score`.
- Il le compare avec le score stocké en mémoire.
- Si le nouveau score est strictement inférieur à l'ancien (`new_score < old_score`), une alerte est levée.
- Le nouveau score met à jour l'état.

### 3. Broadcasting et Affichage (Phoenix LiveView)
- L'alerte déclenche un appel `Phoenix.PubSub.broadcast(Axon.PubSub, "system_alerts", {:security_degraded, repo, old_score, new_score})`.
- Le dashboard Web s'abonne à ce topic.
- Lorsqu'il reçoit cet événement, il ajoute une notification visuelle (Toast / Alert Box rouge) dans le Dashboard pour prévenir le développeur en temps réel.

## Modifications Prévues
1. `src/dashboard/lib/axon_dashboard/bridge_client.ex` (ou équivalent gérant la socket) : Ajouter la gestion de l'état `security_scores` et la logique de comparaison.
2. `src/dashboard/lib/axon_dashboard_web/live/status_live.ex` : Souscrire au PubSub et intégrer les alertes dans le HTML.
3. `ROADMAP.md` : Marquer l'Audit Proactif comme terminé.