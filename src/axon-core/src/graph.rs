use lbug::{Connection, Database, SystemConfig};
use std::path::Path;
use anyhow::{Context, Result};
use std::sync::Arc;

pub struct GraphStore {
    db: Arc<Database>,
}

impl GraphStore {
    pub fn new(db_path: &str) -> Result<Self> {
        // S'assure que le répertoire existe
        if !Path::new(db_path).exists() {
            std::fs::create_dir_all(db_path)?;
        }

        let config = SystemConfig::default();
        let db = Database::new(db_path, config)?;
        let arc_db = Arc::new(db);

        let store = Self { db: arc_db };
        store.init_schema()?;

        Ok(store)
    }

    pub fn get_connection(&self) -> Result<Connection> {
        Ok(Connection::new(self.db.as_ref())?)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.get_connection()?;
        
        // Création des tables de nœuds
        conn.query("CREATE NODE TABLE IF NOT EXISTS File (path STRING, PRIMARY KEY (path))")
            .context("Failed to create File table")?;
            
        conn.query("CREATE NODE TABLE IF NOT EXISTS Symbol (name STRING, kind STRING, PRIMARY KEY (name))")
            .context("Failed to create Symbol table")?;

        // Création de la table de relation
        conn.query("CREATE REL TABLE IF NOT EXISTS CONTAINS (FROM File TO Symbol)")
            .context("Failed to create CONTAINS rel table")?;

        Ok(())
    }

    pub fn insert_file_symbols(&self, path: &str, symbols: &[crate::parser::Symbol]) -> Result<()> {
        let conn = self.get_connection()?;
        
        // 1. Insertion du fichier
        let query_file = format!("MERGE (f:File {{path: '{}'}})", path);
        conn.query(&query_file)?;

        // 2. Insertion des symboles et des relations
        // Dans une implémentation de production, on utiliserait des Prepared Statements (Query parameters)
        // pour des raisons de performance et de sécurité (évite les injections).
        // Ici, pour le PoC rapide, on construit la requête.
        for sym in symbols {
            let safe_name = sym.name.replace("'", "''");
            let safe_kind = sym.kind.replace("'", "''");
            
            let query_sym = format!(
                "MERGE (s:Symbol {{name: '{}', kind: '{}'}})",
                safe_name, safe_kind
            );
            conn.query(&query_sym)?;

            let query_rel = format!(
                "MATCH (f:File {{path: '{}'}}), (s:Symbol {{name: '{}'}}) MERGE (f)-[:CONTAINS]->(s)",
                path, safe_name
            );
            conn.query(&query_rel)?;
        }

        Ok(())
    }
}
