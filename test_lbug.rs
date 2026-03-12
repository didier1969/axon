use lbug::{Database, Connection, SystemConfig};
fn main() {
    let db = Database::new(":memory:", SystemConfig::default()).unwrap();
    let conn = Connection::new(&db).unwrap();
    let mut result = conn.query("RETURN 1").unwrap();
    if let Some(row) = result.next() {
        println!("Value: {}", row[0]);
    }
}
