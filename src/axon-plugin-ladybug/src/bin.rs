use lbug::{Connection, Database, SystemConfig};

fn main() {
    let config = SystemConfig::default();
    let db = Database::new("/tmp/test_lbug", config).unwrap();
    let conn = Connection::new(&db).unwrap();
    let mut stmt = conn.prepare("MATCH (n:Person {name: $name}) RETURN n").unwrap();
    let mut params = std::collections::HashMap::new();
    params.insert("name", lbug::Value::String("Alice".to_string()));
    // Let's see if this compiles to guess the API
    conn.execute(&mut stmt, params).unwrap();
}
