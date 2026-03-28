use lbug::{Connection, Database, SystemConfig, Value};

#[test]
fn test_prepared_statement() {
    let config = SystemConfig::default();
    let db = Database::new("/tmp/test_lbug_db_123", config).unwrap();
    let conn = Connection::new(&db).unwrap();
    
    // Just guessing the API based on typical Kuzu bindings
    let mut stmt = conn.prepare("MATCH (n:Person {name: $name}) RETURN n").unwrap();
    
    let mut params = std::collections::HashMap::new();
    params.insert("name", Value::String("Alice".to_string()));
    
    let _result = conn.execute(&mut stmt, params).unwrap();
}
