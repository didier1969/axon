use serde::{Deserialize, Serialize};
use std::os::unix::net::UnixListener;
use std::io::Write;
use std::fs;
use std::path::Path;
use anyhow::Result;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BridgeEvent {
    FileIndexed { path: String, symbol_count: usize },
    ScanComplete { total_files: usize, duration_ms: u64 },
    Heartbeat,
}

pub struct Bridge {
    socket_path: String,
}

impl Bridge {
    pub fn new(path: &str) -> Self {
        Self {
            socket_path: path.to_string(),
        }
    }

    pub fn start_server(&self) -> Result<()> {
        let path = Path::new(&self.socket_path);
        
        // Nettoyage de la socket si elle existe
        if path.exists() {
            fs::remove_file(path)?;
        }

        println!("Bridge UDS listening on {}", self.socket_path);
        let listener = UnixListener::bind(path)?;

        // Ce serveur est minimal : il accepte une connexion et stream les events
        // Pour Axon v2, on suppose un seul dashboard connecté
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(mut stream) => {
                        println!("Dashboard connected to Bridge");
                        // Pour le moment, on garde la connexion ouverte et on attend
                        // Dans une version réelle, on utiliserait un channel pour envoyer les events
                        let _ = stream.write_all(b"Axon Bridge Ready\n");
                    }
                    Err(err) => eprintln!("Bridge connection error: {}", err),
                }
            }
        });

        Ok(())
    }

    pub fn send_event(path: &str, event: BridgeEvent) -> Result<()> {
        let socket_path = Path::new(path);
        if !socket_path.exists() {
            return Ok(()); // Pas de dashboard, pas d'envoi
        }

        // On tente une connexion éphémère ou on utilise un singleton (à améliorer)
        // Pour ce prototype, on va juste logger
        println!("Bridge Event: {:?}", event);
        
        Ok(())
    }
}
