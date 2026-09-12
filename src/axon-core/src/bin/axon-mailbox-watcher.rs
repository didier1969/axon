//! REQ-AXO-902548 / DEC-AXO-902549: Watcher autonome de boîte aux lettres Axon.
//!
//! Écoute les signaux PostgreSQL NOTIFY (`axon_mailbox_wake`, `axon_mailbox`)
//! et les datagrammes Unix Domain Socket (`/tmp/axon_mailbox.sock`).
//! Émet une alerte formatée sur stdout pour déclencher le réveil réactif (Reactive Wakeup)
//! de la session de codage LLM sans polling applicatif.

use futures_util::StreamExt;
use serde_json::Value;
use std::env;
use std::io::Write;
use std::time::Duration;
use tokio::net::UnixDatagram;
use tokio_postgres::{AsyncMessage, NoTls};

#[derive(Debug, Clone)]
struct WatcherConfig {
    project_filter: Option<String>,
    exit_once: bool,
    pg_host: String,
    pg_port: u16,
    pg_db: String,
    pg_user: String,
    sock_path: String,
}

impl WatcherConfig {
    fn from_args() -> Self {
        let args: Vec<String> = env::args().collect();
        let mut project_filter = None;
        let mut exit_once = false;
        let mut pg_host = env::var("PGHOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let mut pg_port = env::var("PGPORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(44144);
        let mut pg_db = env::var("PGDATABASE").unwrap_or_else(|_| "axon_live".to_string());
        let mut pg_user = env::var("PGUSER").unwrap_or_else(|_| "axon".to_string());
        let mut sock_path =
            env::var("AXON_MAILBOX_SOCK").unwrap_or_else(|_| "/tmp/axon_mailbox.sock".to_string());

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--project" | "-p" if i + 1 < args.len() => {
                    project_filter = Some(args[i + 1].clone());
                    i += 2;
                }
                "--once" => {
                    exit_once = true;
                    i += 1;
                }
                "--host" if i + 1 < args.len() => {
                    pg_host = args[i + 1].clone();
                    i += 2;
                }
                "--port" if i + 1 < args.len() => {
                    if let Ok(p) = args[i + 1].parse() {
                        pg_port = p;
                    }
                    i += 2;
                }
                "--db" if i + 1 < args.len() => {
                    pg_db = args[i + 1].clone();
                    i += 2;
                }
                "--user" if i + 1 < args.len() => {
                    pg_user = args[i + 1].clone();
                    i += 2;
                }
                "--sock" if i + 1 < args.len() => {
                    sock_path = args[i + 1].clone();
                    i += 2;
                }
                _ => {
                    i += 1;
                }
            }
        }

        Self {
            project_filter,
            exit_once,
            pg_host,
            pg_port,
            pg_db,
            pg_user,
            sock_path,
        }
    }
}

fn emit_alert(to: &str, from: &str, message_id: &str, priority: &str, subject: &str) {
    println!(
        "\n[AXON_MAILBOX_ALERT] Projet: {to} | De: {from} | Message: {message_id} | Priorité: {priority}\n\
         Objet: {subject}\n\
         Notification: Message entrant disponible dans la boîte aux lettres Axon.\n\
         Consigne: Exécuter `mcp_inbox_read(project: \"{to}\")` dès la fin du travail en cours."
    );
    let _ = std::io::stdout().flush();
}

fn handle_json_notification(config: &WatcherConfig, parsed: &Value) -> bool {
    let to = parsed.get("to").and_then(Value::as_str).unwrap_or("");
    if to.is_empty() {
        return false;
    }

    if let Some(ref filter) = config.project_filter {
        if filter != "*" && filter != to {
            return false;
        }
    }

    let from = parsed
        .get("from")
        .and_then(Value::as_str)
        .unwrap_or("inconnu");
    let message_id = parsed
        .get("message_id")
        .or_else(|| parsed.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("n/a");
    let priority = parsed
        .get("priority")
        .and_then(Value::as_str)
        .unwrap_or("normal");
    let subject = parsed
        .get("subject")
        .and_then(Value::as_str)
        .unwrap_or("Sans objet");

    emit_alert(to, from, message_id, priority, subject);
    true
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = WatcherConfig::from_args();

    eprintln!(
        "[axon-mailbox-watcher] Démarrage de l'écoute sur PG {}:{} ({}) et UDS {}",
        config.pg_host, config.pg_port, config.pg_db, config.sock_path
    );
    if let Some(ref p) = config.project_filter {
        eprintln!("[axon-mailbox-watcher] Filtre actif pour le projet: `{p}`");
    } else {
        eprintln!("[axon-mailbox-watcher] Écoute globale (tous les projets)");
    }

    let (alert_tx, mut alert_rx) = tokio::sync::mpsc::channel::<Value>(100);

    // 1. Tâche d'écoute Unix Datagram Socket
    let sock_path_clone = config.sock_path.clone();
    let alert_tx_uds = alert_tx.clone();
    tokio::spawn(async move {
        let _ = std::fs::remove_file(&sock_path_clone);
        if let Ok(uds) = UnixDatagram::bind(&sock_path_clone) {
            let mut buf = vec![0u8; 4096];
            loop {
                if let Ok((len, _)) = uds.recv_from(&mut buf).await {
                    if let Ok(val) = serde_json::from_slice::<Value>(&buf[..len]) {
                        let _ = alert_tx_uds.send(val).await;
                    }
                }
            }
        }
    });

    // 2. Tâche d'écoute PostgreSQL NOTIFY
    let config_pg = config.clone();
    let alert_tx_pg = alert_tx.clone();
    tokio::spawn(async move {
        loop {
            let conn_str = format!(
                "host={} port={} dbname={} user={}",
                config_pg.pg_host, config_pg.pg_port, config_pg.pg_db, config_pg.pg_user
            );
            match tokio_postgres::connect(&conn_str, NoTls).await {
                Ok((client, mut connection)) => {
                    let (notify_tx, mut notify_rx) =
                        tokio::sync::mpsc::channel::<tokio_postgres::Notification>(100);
                    tokio::spawn(async move {
                        let stream =
                            futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
                        tokio::pin!(stream);
                        while let Some(msg) = stream.next().await {
                            if let Ok(AsyncMessage::Notification(n)) = msg {
                                let _ = notify_tx.send(n).await;
                            }
                        }
                    });

                    if client
                        .batch_execute("LISTEN axon_mailbox; LISTEN axon_mailbox_wake;")
                        .await
                        .is_ok()
                    {
                        while let Some(notif) = notify_rx.recv().await {
                            if let Ok(val) = serde_json::from_str::<Value>(notif.payload()) {
                                let _ = alert_tx_pg.send(val).await;
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("[axon-mailbox-watcher] Reconnexion PG dans 2s suite à: {err}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    });

    // Boucle principale de réception et filtrage
    while let Some(msg) = alert_rx.recv().await {
        if handle_json_notification(&config, &msg) && config.exit_once {
            break;
        }
    }

    Ok(())
}
