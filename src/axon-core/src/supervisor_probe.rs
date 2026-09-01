//! REQ-AXO-902585 (défaut 3) — lire l'état des rôles auprès du superviseur.
//!
//! RÉUTILISE : néant — vérifié via `axon query "sonde HTTP superviseur
//! process-compose liste des processus"` (aucun symbole couvrant). Les deux
//! voisins ne conviennent pas et c'est mesuré, pas supposé :
//! `indexer_health_http.rs` est un SERVEUR (le brain EXPOSE ses sondes pour que
//! process-compose l'interroge) — sens inverse ; `axonctl::probe_supervisor_healthy`
//! scanne la table de processus de l'OS (`ps`/cmdline), n'appelle aucune API et ne
//! porte ni compteur de redémarrages ni âge. Le motif de transport `TcpStream` nu
//! est repris d'`indexer_health_http.rs` (côté test), pas réinventé.
//!
//! Le battement PG dit qu'UN processus a écrit récemment ; il ne dit jamais que
//! LE MÊME processus tient. Mesuré le 2026-09-01 : après un promote échoué,
//! `process-compose` relançait `axon-indexer` en boucle — `restarts` 11→13 en 40 s,
//! un pid neuf à chaque tour, chacun vivant ~8 s — pendant que `promote_status`
//! rendait `phase: clean` et `indexer_alive: pass`. Chaque instance éphémère écrit
//! son battement avant de mourir : une fenêtre de fraîcheur de 30 s ne peut pas
//! distinguer un processus sain d'une SUITE de processus qui meurent.
//!
//! Deux mesures la révèlent, et aucune n'est un battement : le COMPTEUR de
//! redémarrages du superviseur, et la STABILITÉ du pid. Seul le superviseur les
//! porte — d'où cette sonde.
//!
//! Transport : `TcpStream` nu. Ni `reqwest` ni `ureq` ne sont des dépendances
//! directes, et `curl` est écarté délibérément : le brain tourne sous
//! process-compose, qui hérite d'un PATH non-devenv — c'est la classe de panne que
//! `REQ-AXO-902345` documente en production.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::Value;

pub const SUPERVISOR_CONNECT_TIMEOUT_MS: u64 = 500;
pub const SUPERVISOR_READ_TIMEOUT_MS: u64 = 1_500;
pub const SUPERVISOR_MAX_BODY_BYTES: usize = 1 << 20;

/// REQ-AXO-902585 — seuils de détection. Nommés et constants : ils sont un CHOIX,
/// pas une mesure, et doivent pouvoir être discutés sans être cherchés.
pub const SUPERVISOR_RESTART_LOOP_MIN_RESTARTS: i64 = 3;
pub const SUPERVISOR_YOUNG_PROCESS_MS: i64 = 60_000;

/// Un rôle tel que le superviseur le décrit.
///
/// ⚠ La forme est celle MESURÉE sur `process-compose` 0.5, pas celle qu'on
/// suppose : l'enveloppe est `{"data":[…]}` et non un tableau nu, `is_ready` est
/// une CHAÎNE (`"Ready"` / `"-"`) et non un booléen, et `age` est en
/// NANOSECONDES. Un parseur écrit sur les suppositions rendrait « aucun
/// processus » et se croirait correct.
#[derive(Debug, Clone, PartialEq)]
pub struct SupervisorProcess {
    pub name: String,
    pub status: String,
    pub is_ready: String,
    pub pid: i64,
    pub restarts: i64,
    pub exit_code: i64,
    pub is_running: bool,
    pub age_ns: i64,
}

impl SupervisorProcess {
    pub fn age_ms(&self) -> i64 {
        self.age_ns / 1_000_000
    }
}

/// PURE. Accepte les DEUX enveloppes : `{"data":[…]}` (mesurée) et un tableau nu
/// (tolérance de version).
///
/// Une forme non reconnue rend `Err`, JAMAIS `Ok(vec![])` : « parsé, aucun rôle de
/// ce nom » et « corps illisible » sont deux verdicts différents, et les confondre
/// referait le défaut que cette tranche corrige.
pub fn parse_processes(body: &str) -> Result<Vec<SupervisorProcess>, String> {
    let parsed: Value =
        serde_json::from_str(body).map_err(|e| format!("supervisor body is not JSON: {e}"))?;
    let items = match parsed.get("data").and_then(Value::as_array) {
        Some(items) => items,
        None => parsed
            .as_array()
            .ok_or_else(|| "supervisor body is neither {data:[…]} nor a bare array".to_string())?,
    };
    Ok(items
        .iter()
        .filter_map(|p| {
            Some(SupervisorProcess {
                name: p.get("name")?.as_str()?.to_string(),
                status: p
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                is_ready: p
                    .get("is_ready")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                pid: p.get("pid").and_then(Value::as_i64).unwrap_or(0),
                restarts: p.get("restarts").and_then(Value::as_i64).unwrap_or(0),
                exit_code: p.get("exit_code").and_then(Value::as_i64).unwrap_or(0),
                is_running: p
                    .get("is_running")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                age_ns: p.get("age").and_then(Value::as_i64).unwrap_or(0),
            })
        })
        .collect())
}

/// PURE. L'override arrive en PARAMÈTRE, jamais lu depuis l'environnement ici :
/// c'est ce qui rend la fonction testable sans verrou global sur l'env
/// (`REQ-AXO-902261`).
///
/// Défaut **`live`** et non `dev` : sonder le port de dev en servant le live
/// donnerait une image fausse, et confiante. Valeurs alignées sur
/// `scripts/lib/axon-supervisor.sh::axon_pc_port_for_instance`, seule source de
/// vérité — un test d'anti-dérive le vérifie.
pub fn supervisor_port_from(raw_override: Option<String>, instance_kind: &str) -> u16 {
    if let Some(raw) = raw_override {
        if let Ok(port) = raw.trim().parse::<u16>() {
            if port != 0 {
                return port;
            }
        }
    }
    match instance_kind.trim() {
        "dev" => 8081,
        _ => 8080,
    }
}

pub fn supervisor_addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// Requête HTTP/1.1 minimale, bornée dans le temps ET dans le volume.
///
/// Toute erreur remonte en `Err(String)` portant sa cause, et cette cause est
/// rendue telle quelle dans `observed` : une sonde injoignable doit se DIRE, pas
/// se deviner.
pub fn fetch_processes(
    addr: SocketAddr,
    connect: Duration,
    read: Duration,
) -> Result<String, String> {
    let mut stream =
        TcpStream::connect_timeout(&addr, connect).map_err(|e| format!("connect {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(read))
        .map_err(|e| format!("set_read_timeout: {e}"))?;
    stream
        .set_write_timeout(Some(read))
        .map_err(|e| format!("set_write_timeout: {e}"))?;
    let request = format!(
        "GET /processes HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write request: {e}"))?;

    let mut raw = Vec::new();
    // Plafonné plutôt que fondé sur `Content-Length` : un superviseur qui mentirait
    // sur sa taille ne doit pas pouvoir faire grossir un outil de LECTURE.
    stream
        .take(SUPERVISOR_MAX_BODY_BYTES as u64)
        .read_to_end(&mut raw)
        .map_err(|e| format!("read response: {e}"))?;
    let text = String::from_utf8_lossy(&raw).into_owned();

    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed HTTP response (no header/body split)".to_string())?;
    let status_line = head.lines().next().unwrap_or("");
    if !status_line.contains(" 200") {
        return Err(format!("supervisor answered `{status_line}`"));
    }
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Corps CAPTURÉ du vrai serveur, collé verbatim. C'est lui qui casserait un
    /// parseur écrit sur des suppositions.
    const CORPS_REEL: &str = r#"{"data":[{"age":2561906634062,"cpu":0.5,"exit_code":0,"has_ready_probe":true,"is_elevated":false,"is_ready":"Ready","is_running":true,"mem":1234,"name":"axon-indexer","namespace":"default","password_provided":false,"pid":655449,"restarts":0,"status":"Running","system_time":"1h"}]}"#;

    #[test]
    fn parses_the_data_enveloped_shape() {
        let procs = parse_processes(CORPS_REEL).expect("le corps réel doit être parsé");
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].name, "axon-indexer");
        assert_eq!(procs[0].status, "Running");
        assert_eq!(procs[0].pid, 655449);
    }

    #[test]
    fn parses_a_bare_array_shape() {
        // Tolérance de version : une autre release pourrait rendre le tableau nu.
        let procs = parse_processes(r#"[{"name":"axon-brain","status":"Running","pid":1}]"#)
            .expect("tableau nu accepté");
        assert_eq!(procs.len(), 1);
    }

    #[test]
    fn an_unrecognised_body_is_an_error_not_an_empty_list() {
        // « parsé, aucun rôle » et « corps illisible » sont deux verdicts.
        for corps in [r#"{"processes":[]}"#, "null", "<html>oops</html>"] {
            assert!(
                parse_processes(corps).is_err(),
                "un corps non reconnu doit être une ERREUR : {corps}"
            );
        }
    }

    #[test]
    fn is_ready_is_a_string_and_age_is_nanoseconds() {
        // Les deux pièges de forme, épinglés : un `as_bool()` sur `is_ready` ou un
        // `age` lu en millisecondes rendraient une image fausse et confiante.
        let procs = parse_processes(CORPS_REEL).unwrap();
        assert_eq!(procs[0].is_ready, "Ready");
        assert_eq!(procs[0].age_ns, 2_561_906_634_062);
        assert_eq!(procs[0].age_ms(), 2_561_906);
    }

    #[test]
    fn the_default_port_follows_the_instance_kind() {
        assert_eq!(supervisor_port_from(None, "live"), 8080);
        assert_eq!(supervisor_port_from(None, "dev"), 8081);
        // Défaut LIVE sur une valeur inconnue : sonder le dev en servant le live
        // donnerait une image fausse.
        assert_eq!(supervisor_port_from(None, "n'importe quoi"), 8080);
    }

    #[test]
    fn a_malformed_port_override_falls_back_instead_of_probing_zero() {
        assert_eq!(supervisor_port_from(Some("  ".into()), "live"), 8080);
        assert_eq!(supervisor_port_from(Some("abc".into()), "live"), 8080);
        assert_eq!(supervisor_port_from(Some("0".into()), "live"), 8080);
        assert_eq!(supervisor_port_from(Some(" 9999 ".into()), "live"), 9999);
    }

    /// REQ-AXO-902585 — anti-dérive : la convention de port vit dans le bash, cette
    /// copie Rust doit rester alignée. `PIL-AXO-001` — une seule vérité.
    #[test]
    fn the_port_table_still_matches_the_shell_source_of_truth() {
        let shell = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/lib/axon-supervisor.sh"
        ))
        .expect("la source de vérité shell doit être lisible");
        assert!(
            shell.contains("8080") && shell.contains("8081"),
            "les deux ports doivent encore figurer dans axon-supervisor.sh"
        );
    }

    fn faux_superviseur(corps: &'static str) -> SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind éphémère");
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut tampon = [0u8; 1024];
                let _ = sock.read(&mut tampon);
                let reponse = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    corps.len(),
                    corps
                );
                let _ = sock.write_all(reponse.as_bytes());
            }
        });
        addr
    }

    #[test]
    fn fetch_processes_reads_a_canned_supervisor_response() {
        let addr = faux_superviseur(CORPS_REEL);
        let body = fetch_processes(
            addr,
            Duration::from_millis(SUPERVISOR_CONNECT_TIMEOUT_MS),
            Duration::from_millis(SUPERVISOR_READ_TIMEOUT_MS),
        )
        .expect("le faux superviseur répond");
        assert_eq!(parse_processes(&body).unwrap().len(), 1);
    }

    #[test]
    fn a_refused_connection_is_an_error_within_the_timeout() {
        // Un port lié puis libéré : personne n'écoute, la connexion est refusée.
        let addr = {
            let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            l.local_addr().unwrap()
        };
        let debut = std::time::Instant::now();
        let erreur = fetch_processes(
            addr,
            Duration::from_millis(SUPERVISOR_CONNECT_TIMEOUT_MS),
            Duration::from_millis(SUPERVISOR_READ_TIMEOUT_MS),
        )
        .expect_err("connexion refusée");
        assert!(erreur.contains("connect"), "la cause est nommée : {erreur}");
        assert!(debut.elapsed() < Duration::from_secs(3));
    }

    /// LE test qui prouve qu'un superviseur muet ne peut pas figer un outil de
    /// LECTURE : le listener accepte et n'écrit jamais rien.
    #[test]
    fn a_hanging_supervisor_does_not_hang_the_reader() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let _garde = listener.accept();
            std::thread::sleep(Duration::from_secs(10));
        });
        let debut = std::time::Instant::now();
        let _ = fetch_processes(
            addr,
            Duration::from_millis(SUPERVISOR_CONNECT_TIMEOUT_MS),
            Duration::from_millis(SUPERVISOR_READ_TIMEOUT_MS),
        );
        assert!(
            debut.elapsed() < Duration::from_secs(3),
            "la sonde doit rendre la main : {:?}",
            debut.elapsed()
        );
    }
}
