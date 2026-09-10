// Copyright (c) Didier Stadelmann. All rights reserved.

//! Qualification TDD de REQ-AXO-902508:
//! Un projet enregistré APRÈS le boot de l'indexeur reste orphelin jusqu'au redémarrage suivant.
//!
//! Critères d'acceptation :
//! 1. Le résolveur in-RAM (`ProjectCodeResolver`) peut être rafraîchi à chaud sans redémarrage du processus.
//! 2. Un nouveau projet enregistré après le boot est reconnu immédiatement par le résolveur rafraîchi.
//! 3. Un projet retiré du registre n'est plus attribué après rafraîchissement (évite les 96 lignes orphelines de KKD).
//! 4. Le payload de notification `axon_registry_changed` supporte l'opération `delete`.

use std::path::{Path, PathBuf};
use crate::pipeline::project_resolver::{ProjectCodeResolver, ProjectRegistrySnapshot};
use crate::project_meta::CanonicalProjectIdentity;

#[test]
fn c1_resolver_hot_reload_recognizes_newly_registered_project() {
    // Situation mesurée sur OPR : au boot, seul PRP est connu
    let initial_snapshot = ProjectRegistrySnapshot::from_rows(vec![
        ("PRP".to_string(), "/home/dstadel/projects/opv".to_string()),
    ]).unwrap();

    let resolver = ProjectCodeResolver::from_snapshot(initial_snapshot);
    let opr_file = Path::new("/home/dstadel/projects/opv-rd/src/main.rs");

    // Avant enregistrement de OPR, le fichier n'est pas attribué à OPR
    assert!(!resolver.contains_code("OPR"));
    let initial_resolution = resolver.resolve(opr_file);
    // Il n'est pas résolu à OPR
    if let Ok(code) = initial_resolution {
        assert_ne!(code.as_str(), "OPR");
    }

    // À 06h51, OPR est enregistré au registre
    let updated_identities = vec![
        CanonicalProjectIdentity {
            code: "PRP".to_string(),
            project_path: PathBuf::from("/home/dstadel/projects/opv"),
            name: Some("opv".to_string()),
            meta_path: PathBuf::from("/home/dstadel/projects/opv/axon.json"),
        },
        CanonicalProjectIdentity {
            code: "OPR".to_string(),
            project_path: PathBuf::from("/home/dstadel/projects/opv-rd"),
            name: Some("opv-rd".to_string()),
            meta_path: PathBuf::from("/home/dstadel/projects/opv-rd/axon.json"),
        },
    ];

    let count = resolver.refresh_from_identities(updated_identities)
        .expect("refresh should succeed");
    assert_eq!(count, 2);

    // Après rafraîchissement à chaud, OPR est reconnu immédiatement
    assert!(resolver.contains_code("OPR"));
    let resolved = resolver.resolve(opr_file).expect("OPR file must resolve");
    assert_eq!(resolved.as_str(), "OPR");
}

#[test]
fn c2_resolver_hot_reload_removes_deleted_project() {
    // Situation mesurée sur KKD : KKD était présent au boot
    let initial_snapshot = ProjectRegistrySnapshot::from_rows(vec![
        ("KKD".to_string(), "/home/dstadel/projects/kkd".to_string()),
        ("AXO".to_string(), "/home/dstadel/projects/axon".to_string()),
    ]).unwrap();

    let resolver = ProjectCodeResolver::from_snapshot(initial_snapshot);
    let kkd_file = Path::new("/home/dstadel/projects/kkd/lib/app.ex");

    assert!(resolver.contains_code("KKD"));
    assert_eq!(resolver.resolve(kkd_file).unwrap().as_str(), "KKD");

    // À 08h14, KKD est retiré du registre
    let updated_identities = vec![
        CanonicalProjectIdentity {
            code: "AXO".to_string(),
            project_path: PathBuf::from("/home/dstadel/projects/axon"),
            name: Some("axon".to_string()),
            meta_path: PathBuf::from("/home/dstadel/projects/axon/axon.json"),
        },
    ];

    resolver.refresh_from_identities(updated_identities)
        .expect("refresh should succeed");

    // Après rafraîchissement, KKD n'est plus attribué
    assert!(!resolver.contains_code("KKD"));
    assert!(resolver.resolve(kkd_file).is_err());
}
