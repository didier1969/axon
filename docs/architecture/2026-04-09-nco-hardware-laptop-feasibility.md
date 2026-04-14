# Rapport de Faisabilité : NCO sur Hardware Edge (Laptop 8Go VRAM)

**Statut :** Plan d'Exécution Matériel
**Contexte :** Entraînement d'un modèle d'Optimisation Combinatoire Neurale (NCO) pour le Job-Shop Scheduling Problem (JSSP) d'HexaFactory sur un ordinateur portable équipé d'une carte graphique de 8 Go de VRAM.

---

## 1. Faisabilité Technique : Validée (100%)

La réponse est extrêmement positive. En 2025-2026, l'architecture des réseaux de neurones sur graphes (GNN) et des mécanismes d'attention pour l'ordonnancement est **totalement compatible avec 8 Go de VRAM**.

Contrairement aux Large Language Models (LLMs) qui nécessitent des dizaines de gigaoctets pour stocker leurs milliards de paramètres, un modèle NCO pour le JSSP est structurellement "petit" (souvent moins de 50 millions de paramètres, soit moins de 200 Mo sur le disque).

Le goulot d'étranglement n'est pas la taille du modèle, mais la taille des **batchs d'usines** (combien d'usines virtuelles vous envoyez au GPU en même temps pour l'entraînement). Avec 8 Go de VRAM, vous pouvez largement faire tenir des batchs de 64 à 128 "petites" usines générées par `dataset.ex` à chaque itération.

## 2. Estimation du Temps de Traitement (Training Time)

Sur une carte graphique type RTX 3060 / 4060 Laptop (8 Go VRAM), le profil d'entraînement en Apprentissage par Renforcement (RL - type PPO) pour converger vers un agent capable de déjouer les setups destructeurs est le suivant :

*   **Premiers Signes d'Intelligence : 4 à 6 heures.**
    Dans ce laps de temps, l'agent aura compris les règles du jeu (ne pas faire déborder les buffers) et commencera à battre de simples règles heuristiques (comme "Faire le job le plus court d'abord").
*   **Convergence Solide (Optimum Local) : 12 à 24 heures.**
    Le modèle commencera à maîtriser la stratégie macroscopique (grouper les ordres similaires pour éviter les 40% de perte de setup).
*   **Maîtrise SOTA (State of the Art) : 36 à 48 heures.**
    C'est le temps nécessaire pour que l'algorithme "explore" suffisamment de cas extrêmes (Edge Cases générés par votre seed Elixir) et devienne robuste.

*Note : Si vous intégrez de la stochasticité (des temps de setup qui varient aléatoirement de +/- 10%), le temps d'entraînement devra être doublé (72 heures) car le modèle devra apprendre à gérer le risque.*

## 3. Stratégie d'Accélération (Le "Hack" 2026)

Pour diviser ce temps par deux sur votre laptop, la recherche actuelle préconise l'approche du **Transfert d'Échelle (Zero-Shot Generalization)** :

1.  **N'entraînez pas le modèle sur une usine de 200 machines.** (L'entraînement serait trop long, le signal de récompense trop diffus).
2.  **Entraînez-le sur des "Micro-Usines" (Ex: 10 machines, 50 ordres).**
    Le temps d'entraînement sur votre laptop passera de 48 heures à **4 ou 6 heures maximum**. Sur un graphe aussi petit, le réseau de neurones apprend très vite les "lois de la physique" (les setups, les goulots d'étranglement).
3.  **L'Inférence (La Magie du GNN) :**
    Une fois le modèle entraîné sur la micro-usine, vous figez ses poids mathématiques. Dans HexaCore, vous lui injectez votre vraie usine de 200 machines. Grâce à sa nature topologique (Graph Neural Network), le modèle va **généraliser sa logique mathématique à la grande échelle instantanément**. Ses performances ne s'effondreront pas, car un goulot d'étranglement mathématique reste le même qu'il y ait 10 ou 200 nœuds autour de lui.

## 4. Recommandations Matérielles Locales (Laptop)

Puisque l'entraînement va durer entre 12h et 48h en continu :
*   **Thermal Throttling :** Votre laptop va chauffer. Pour éviter qu'il ne bride la carte graphique, surélevez-le ou utilisez un support ventilé.
*   **Mixed Precision (FP16) :** Dans votre code Python (PyTorch/JAX), activez impérativement l'Automatic Mixed Precision (`torch.cuda.amp`). Cela divisera par deux l'empreinte mémoire dans vos 8 Go de VRAM et accélérera les calculs matriciels de **1.5x à 2x** grâce aux Tensor Cores de votre carte graphique.
*   **Batch Size :** Démarrez avec des batchs de 64 usines. Si votre VRAM ne sature pas (vérifiez avec `nvidia-smi`), montez à 128. Un batch plus grand stabilise le gradient du Reinforcement Learning et empêche le modèle "d'oublier" ce qu'il vient d'apprendre.