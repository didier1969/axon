# Recherche : Neural Combinatorial Optimization (NCO) vs Métaheuristiques (JSSP)

**Statut :** Recherche Fondamentale (Indépendant d'Axon)
**Contexte :** Évaluation de la viabilité et de l'avantage de l'Optimisation Combinatoire Neurale (NCO) par projection latente (Embeddings) face aux algorithmes classiques (Algorithmes Génétiques - GA, et Recuit Simulé - SA) pour la résolution du Job-Shop Scheduling Problem (JSSP).

---

## 1. Le Paradigme : Espace Discret vs Espace Latent

La différence fondamentale entre les métaheuristiques classiques (GA, SA) et la NCO réside dans la géométrie de l'espace d'exploration.

### A. Algorithmes Génétiques et Recuit Simulé (Recherche Discrète)
*   **Mécanique :** Ces algorithmes opèrent directement dans l'espace des solutions discrètes (des permutations d'opérations sur des machines).
*   **La Limite Physique :** Ils sont structurellement "aveugles". Pour évaluer la qualité d'une permutation (makespan), ils doivent la simuler entièrement. À chaque nouvelle instance du JSSP, l'algorithme repart de zéro et doit évaluer des millions de combinaisons pour converger vers un optimum local.

### B. Neural Combinatorial Optimization (Espace Latent Continu)
*   **Mécanique :** La NCO utilise des réseaux de neurones sur graphes (GNN) pour encoder le problème (opérations, machines, dépendances) dans un espace vectoriel continu (Embeddings).
*   **L'Avantage Physique :** L'algorithme d'optimisation (souvent piloté par Reinforcement Learning) apprend une "politique de routage". Il ne teste pas des permutations au hasard ; il navigue dans l'espace latent en suivant un gradient de probabilité qui le guide mathématiquement vers la séquence optimale.

---

## 2. Les Avantages Structurels des Embeddings pour le JSSP

L'utilisation de la projection vectorielle pour dégrossir les problèmes de JSSP extrêmement complexes présente trois avantages mécaniques stricts.

### 2.1. Temps d'Inférence (Millisecondes vs Heures)
L'entraînement d'un modèle NCO est coûteux en amont. Cependant, une fois le problème projeté dans l'espace latent, le modèle génère une solution quasi-optimale par **inférence directe**.
*   **SA / GA :** Prennent des dizaines de minutes ou des heures pour converger sur de grandes instances industrielles.
*   **NCO :** Produit le planning en quelques millisecondes (O(1) ou O(N) en temps d'inférence), rendant le ré-ordonnancement en temps réel (Edge Computing) possible en cas de panne machine.

### 2.2. La Généralisation Structurelle (Zero-Shot Transfer)
Le défaut majeur du Recuit Simulé et des Algorithmes Génétiques est leur incapacité à apprendre de leurs résolutions précédentes.
*   En NCO, l'embedding capture la **topologie du graphe disjonctif**. Le modèle apprend à identifier mathématiquement ce qu'est un "goulot d'étranglement" (bottleneck) indépendamment du nom de la machine.
*   Si le modèle est entraîné sur des ateliers de type A, ses embeddings lui permettent de fournir un excellent ordonnancement sur un atelier de type B instantanément, car les structures de graphe sous-jacentes partagent la même signature dans l'espace latent.

### 2.3. Gestion Intuitive de la Stochasticité (Pannes et Retards)
Dans un environnement de production réel, les durées des tâches varient (Stochastic JSSP). Les algorithmes génétiques doivent multiplier les scénarios de simulation pour trouver une solution robuste, ce qui multiplie le temps de calcul.
Les architectures NCO récentes (2025-2026) intègrent des mécanismes d'attention dans leurs embeddings, ce qui permet au vecteur latent de se concentrer nativement sur les machines présentant la plus grande variance, produisant un planning robuste par conception.

---

## 3. Recommandation d'Ingénierie : L'Hybridation (Neural-Heuristic)

Les recherches SOTA démontrent que la NCO ne vise pas nécessairement à éliminer le Recuit Simulé (SA), mais à redéfinir son point de départ.

*   **Le Problème du SA :** Placé aléatoirement dans l'espace des solutions, le SA passe 90% de son temps d'exécution à sortir de bassins d'attraction sous-optimaux.
*   **La Stratégie SOTA :** 
    1.  Utiliser la **NCO (Embeddings)** pour "dégrossir" le sujet de manière déterministe. En quelques millisecondes, le modèle dépose la recherche directement dans le "bassin d'attraction" de l'optimum global.
    2.  Lancer un **Recuit Simulé (SA)** avec un budget de calcul drastiquement réduit (ex: 5% du temps normal) pour "polir" la solution (Recherche Locale) et combler l'écart (Optimality Gap) laissé par le réseau de neurones.

**Conclusion :** Votre intuition est confirmée par l'état de l'art industriel. Utiliser l'embedding pour projeter un problème combinatoire (JSSP) continu permet de contourner le mur de la combinatoire discrète. Le passage d'une optimisation "Search Space" (GA/SA) vers une optimisation "Latent Space" (NCO) est le levier principal de performance pour les systèmes de production à très haute complexité.