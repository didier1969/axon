import duckdb

try:
    conn = duckdb.connect(':memory:')
    conn.execute("CREATE SCHEMA IF NOT EXISTS soll")
    conn.execute("CREATE TABLE IF NOT EXISTS soll.RevisionChange (revision_id VARCHAR, entity_type VARCHAR, entity_id VARCHAR, action VARCHAR, before_json VARCHAR, after_json VARCHAR, created_at BIGINT)")
    
    query = """
    INSERT INTO soll.RevisionChange (revision_id, entity_type, entity_id, action, before_json, after_json, created_at)
    VALUES ('REV-DPG-001', 'requirement', 'REQ-DPG-001', 'create', '{}', '{"acceptance_criteria":"[]","description":"Cloner duckpgq-extension avec tous les sous-modules, en utilisant le protocole HTTPS.","evidence_refs":"[]","metadata":"{}","owner":"","priority":"{\"priority\":\"P1\",\"updated_at\":1775563540606}","status":"current","title":"Cloner via HTTPS"}', 1775563540612)
    """
    
    conn.execute(query)
    print("Query executed successfully!")
    
    query2 = """
    INSERT INTO soll.RevisionChange (revision_id, entity_type, entity_id, action, before_json, after_json, created_at)
    VALUES ('REV-DPG-002', 'pillar', 'PIL-DPG-002', 'create', '{}', '{}', 1775563661627)
    """
    conn.execute(query2)
    print("Query 2 executed successfully!")
except Exception as e:
    print(f"DuckDB Error: {e}")
