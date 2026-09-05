// RÉUTILISE : rien du crate — ces tests verrouillent la RAISON d'un choix de
// concurrence, pas une fonction du crate. Dit tel quel plutôt que déguisé.
//
//! REQ-AXO-902589 (a) — pourquoi le corps du tick de télémétrie part sur le pool
//! bloquant, et pourquoi le rattrapage de ticks est désactivé.
//!
//! ## Ce que ces tests couvrent, et ce qu'ils NE couvrent PAS
//!
//! Ils couvrent le MOTIF : un corps synchrone long dans une tâche `spawn` immobilise
//! le worker et rend la surface non réactive ; le même corps sous `spawn_blocking` ne
//! le fait pas. C'est la justification du correctif, et elle est ici falsifiable.
//!
//! Ils ne couvrent PAS `spawn_runtime_telemetry` lui-même : son corps fait ~240 lignes
//! d'accès PG et n'est pas instanciable sans base. **La preuve du correctif est la
//! mesure post-promote** décrite dans le REQ — rejouer 800 appels HTTP directs et
//! exiger zéro aberrant aligné sur la grille `t%1000 ∈ [70, 95]`. Un test vert ici ne
//! dit rien de cette mesure, et c'est pourquoi les deux existent.

use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Délai, mesuré, avant qu'une tâche voisine ne soit ORDONNANCÉE, pendant qu'un corps
/// synchrone de 200 ms occupe le runtime. `bloquant = true` place ce corps dans
/// `spawn` (le défaut corrigé), `false` dans `spawn_blocking` (le correctif).
///
/// On mesure un DÉLAI et non un booléen à une date : le premier essai testait « la
/// voisine a-t-elle tourné avant 60 ms », et il échouait — sur un runtime mono-thread
/// la voisine tourne bel et bien, mais APRÈS les 200 ms. Un booléen ne voyait pas la
/// différence que tout ce correctif vise.
fn delai_avant_ordonnancement_ms(dans_spawn: bool) -> u128 {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");

    let (tx, rx) = mpsc::channel::<Instant>();

    rt.block_on(async move {
        let t0 = Instant::now();
        if dans_spawn {
            tokio::spawn(async {
                std::thread::sleep(Duration::from_millis(200));
            });
        } else {
            tokio::task::spawn_blocking(|| {
                std::thread::sleep(Duration::from_millis(200));
            });
        }
        tokio::spawn(async move {
            let _ = tx.send(Instant::now());
        });
        // Assez long pour que les deux cas aboutissent : c'est le DÉLAI qui les sépare,
        // pas le fait d'aboutir.
        tokio::time::sleep(Duration::from_millis(400)).await;
        rx.recv()
            .expect("la tâche voisine n'a jamais été ordonnancée")
            .duration_since(t0)
            .as_millis()
    })
}

/// Le défaut, reproduit : un corps synchrone dans `spawn` immobilise le worker, donc
/// la tâche voisine attend qu'il finisse. C'est ce que la surface MCP vit chaque
/// seconde — `help()`, qui ne touche aucune base, bloque comme `status`.
#[test]
fn un_corps_synchrone_dans_spawn_fait_attendre_la_surface() {
    let delai = delai_avant_ordonnancement_ms(true);
    assert!(
        delai >= 150,
        "la voisine a été ordonnancée en {delai} ms : le motif n'est PAS reproduit, et \
         l'assertion du test suivant ne prouverait rien"
    );
}

/// Le correctif : le MÊME corps sous `spawn_blocking` laisse le worker libre.
#[test]
fn le_meme_corps_sous_spawn_blocking_laisse_la_surface_reactive() {
    let delai = delai_avant_ordonnancement_ms(false);
    assert!(
        delai < 100,
        "la voisine a attendu {delai} ms : le corps bloquant n'a pas quitté le runtime"
    );
}

/// `Skip` plutôt que le défaut `Burst`. Si un tick déborde sa période, rattraper les
/// ticks manqués empilerait le blocage au lieu de le résorber : une télémétrie en
/// retard doit sauter une mesure, pas doubler la peine.
#[test]
fn skip_ne_rattrape_pas_les_ticks_manques_contrairement_a_burst() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("runtime");

    rt.block_on(async {
        // Référence — le défaut `Burst` rattrape : après 3 périodes d'absence, trois
        // ticks sont immédiatement disponibles.
        let mut burst = tokio::time::interval(Duration::from_millis(100));
        burst.tick().await; // le premier tick est immédiat, par contrat
        tokio::time::sleep(Duration::from_millis(350)).await;
        let mut rattrapes = 0;
        for _ in 0..3 {
            if tokio::time::timeout(Duration::from_millis(1), burst.tick())
                .await
                .is_ok()
            {
                rattrapes += 1;
            }
        }
        assert!(
            rattrapes >= 2,
            "`Burst` n'a rattrapé que {rattrapes} tick(s) — le contrôle négatif ne tient plus, \
             et l'assertion suivante ne prouverait rien"
        );

        // Le choix retenu — `Skip` ne rend qu'un seul tick, quelle que soit l'absence.
        let mut skip = tokio::time::interval(Duration::from_millis(100));
        skip.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        skip.tick().await;
        tokio::time::sleep(Duration::from_millis(350)).await;
        let mut immediats = 0;
        for _ in 0..3 {
            if tokio::time::timeout(Duration::from_millis(1), skip.tick())
                .await
                .is_ok()
            {
                immediats += 1;
            }
        }
        assert_eq!(
            immediats, 1,
            "`Skip` a rendu {immediats} tick(s) immédiats : le rattrapage n'est pas désactivé"
        );
    });
}
