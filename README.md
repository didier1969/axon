# Axon : La Source de Vérité Structurelle

**Axon** est un moteur d'intelligence de code haute performance conçu pour fournir une vision complète, fiable et actionnable de n'importe quelle base de code. Il transforme le code source en un graphe de connaissances vivant, permettant aux développeurs et aux agents IA d'améliorer et de fiabiliser les systèmes avec une précision chirurgicale.

## 🎯 Vision : Fiabilisation & Transparence
Axon ne se contente pas de chercher du texte ; il cartographie les **intentions** du code.
- **Vision à 360° :** Visualisation instantanée des relations (appels, types, dépendances) à travers tous les langages.
- **Fiabilité par la Preuve :** Chaque diagnostic est ancré dans la réalité physique du système.
- **Amélioration Continue :** Identification proactive des zones non testées, des dérives architecturales et des failles de sécurité.

## 🌐 Le Treillis de Connaissance Global (Phase Apollo)
Axon évolue vers une infrastructure de **Souveraineté Sémantique**. Il ne traite plus vos projets comme des entités isolées, mais comme un **Treillis unique et vivant**.
- **Fédération Cross-Projets :** Analysez les impacts d'une modification à travers l'intégralité de vos dépôts.
- **Ingestion Fantôme :** Une indexation en temps réel à flux continu, avec un impact système quasi nul (<5% CPU).
- **Omniscience Proactive :** Le serveur MCP ne se contente plus de répondre, il synthétise et alerte sur les dérives architecturales.

## 🛡️ L'Infrastructure de Confiance (Nexus Grade)
Axon repose sur des piliers technologiques garantissant une fiabilité absolue :

1.  **Le Cerveau Graphe (LadybugDB) :** Intégration de KuzuDB et HydraDB pour stocker et interroger des relations sémantiques complexes avec une performance native.
2.  **L'Armure de Rendu (LiveView.Witness) :** Une bibliothèque révolutionnaire qui garantit que ce qui est affiché sur le Dashboard correspond physiquement à la vérité du système (Vérification L1/L2/L3 incluant la visibilité réelle et l'absence d'erreurs console).
3.  **Isolation & Coexistence :** 
    - **Zéro Collision :** Utilisation de ports fixes non standards (44127+) et d'identifiants uniques pour coexister avec vos autres projets.
    - **Silence Opérationnel :** Consommation CPU/RAM strictement bridée (70%) pour rester totalement transparent pendant votre travail.

## 🧠 Intelligence Agentique (MCP)
Axon est nativement compatible avec le **Model Context Protocol (MCP)**. Il sert d'interface de connaissance entre le code et les agents IA :
- **Mémoire Structurelle :** Fournit à l'IA le contexte qu'elle ne peut pas deviner, éliminant ainsi les hallucinations architecturales.
- **TDD Sémantique :** L'IA peut désormais vérifier physiquement son travail via le pont de vérité Witness avant de certifier un changement.

## Développement Local
L'environnement de développement de référence est **Nix + Devenv**.

Avant tout build, test ou démarrage de service :

```bash
devenv shell
./scripts/validate-devenv.sh
```

Si le validateur échoue, le shell courant n'est pas l'environnement isolé attendu pour Axon. Les scripts de démarrage et de setup s'appuient désormais sur `devenv shell` comme chemin nominal.

---
© 2025-2026 Didier Stadelmann - L'excellence au service de l'architecture.
