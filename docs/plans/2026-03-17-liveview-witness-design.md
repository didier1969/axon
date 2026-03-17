# Design: LiveView.Witness - Semantic Render Verification & Real-time Observability

## 🎯 Vision
**LiveView.Witness** est une bibliothèque autonome (Standalone Library) pour l'écosystème Phoenix LiveView. Elle résout le problème du "Mensonge de l'IA" et de la "Mort Silencieuse" de l'UI en créant un pont de vérité sémantique entre le navigateur (Client) et le serveur (BEAM).

Elle permet de faire du **TDD Sémantique** : valider que l'intention du serveur est physiquement réalisée dans le moteur de rendu du navigateur (CSS, JS, Assets inclus).

---

## 🏛️ Architecture

Le système est conçu pour être découplé et réutilisable dans n'importe quel projet Phoenix.

### 1. Les Composants Core
*   **`LiveView.Witness.Plug` (The Oracle) :** 
    *   Un Plug Elixir léger servant de point d'entrée "Hors-Bande" (Out-of-Bound).
    *   Reçoit les rapports de crash (500, Déconnexion WebSocket) via une simple requête POST.
    *   Fonctionne même si la LiveView est crashée ou bloquée.
*   **`LiveView.Witness.Hook` (The Inspector) :** 
    *   Un Hook Phoenix JavaScript à enregistrer dans `app.js`.
    *   Écoute les "Contrats" envoyés par le serveur.
    *   Mesure la réalité physique du DOM (visibilité réelle, z-index, temps de rendu).
    *   Intercepte les `console.error` et les échecs de chargement d'assets (404).
*   **`LiveView.Witness.Contract` (The API) :** 
    *   Interface Elixir pour définir des attentes (`expect_ui`).
    *   Gère la file d'attente des "Promesses de Rendu" et valide les certificats renvoyés par le client.

---

## 🔍 Niveaux de Vérification (The 3-Tier Audit)

| Niveau | Nom | Description |
| :--- | :--- | :--- |
| **L1** | **Structurel** | Présence de l'ID, contenu texte exact, nombre d'éléments dans une liste. |
| **L2** | **Physique** | Visibilité réelle (`is_visible?` via `getBoundingClientRect` + `opacity` + `z-index` + occlusion). |
| **L3** | **Santé/Perf** | Erreurs console JS, erreurs réseau (404), temps entre le patch et le rendu final (ms). |

---

## 🚀 Workflow Technique (La Boucle de Vérité)

1.  **Instruction (Serveur) :** Le développeur (ou l'IA) appelle `Witness.expect_ui(socket, "#btn-start", visibility: :visible)`.
2.  **Manifeste (JSON) :** Le serveur pousse un événement `phx-witness:contract` au client contenant les critères de validation.
3.  **Audit (Navigateur) :** Le Hook JS exécute l'inspection physique. Il utilise `document.elementFromPoint` pour s'assurer qu'aucun élément ne masque la cible.
4.  **Certificat (Signature) :** Le navigateur renvoie un `Certificate` :
    *   `status: :ok` -> L'UI est conforme.
    *   `status: :error` -> Détails précis (ex: "ID trouvé mais masqué par `.overlay`", "JS Error: reference error at line 10").
5.  **Validation (TDD) :** En mode test, la suite `LiveView.Witness.Test` bloque (timeout configurable) tant que le certificat n'est pas positif.

---

## 🛡️ Résilience : Le Watchdog OOB

Pour éviter la "Mort Silencieuse" (Page 500), un script minuscule est injecté dans le `root.html.heex`. 
Si Phoenix ne parvient pas à établir la connexion WebSocket ou si le serveur renvoie un code d'erreur HTTP, ce script "siffle" l'alerte vers le `LiveView.Witness.Plug` pour fournir un diagnostic immédiat à l'IA/Développeur.

---

## 💎 Intégration & Portabilité
*   **Namespace :** `LiveView.Witness` (pas de dépendance à Axon).
*   **Installation :** Via `mix.exs`, un Plug à ajouter dans le `endpoint.ex` et un Hook à ajouter dans `app.js`.
*   **Usage IA :** L'agent (LLM) utilise un outil `check_ui_status` qui interroge l'état du dernier certificat reçu.

---

## ✅ Critères de Succès (Definition of Done)
*   [ ] Un crash JS dans le navigateur est immédiatement visible dans les logs Elixir.
*   [ ] Un test TDD peut échouer si un élément est présent dans le HTML mais caché par du CSS (`display: none`).
*   [ ] L'IA peut s'auto-corriger en lisant le rapport d'erreur sémantique envoyé par le navigateur.
