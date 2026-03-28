import re

with open('src/axon-core/src/mcp.rs', 'r') as f:
    content = f.read()

# 1. MERGE (f:File {path: 'X', project_slug: 'Y'}) -> INSERT INTO File (path, project_slug) VALUES ('X', 'Y')
content = re.sub(
    r"MERGE \(f:File \{path: '([^']+)', project_slug: '([^']+)'\}\)",
    r"INSERT INTO File (path, project_slug) VALUES ('\1', '\2')",
    content
)

# 2. MERGE (f:File {path: 'X'}) -> INSERT INTO File (path, project_slug) VALUES ('X', 'global')
content = re.sub(
    r"MERGE \(f:File \{path: '([^']+)'\}\)",
    r"INSERT INTO File (path, project_slug) VALUES ('\1', 'global')",
    content
)

# 3. MERGE (s:Symbol {id: 'X', name: 'Y', kind: 'Z', tested: true/false, is_nif: true/false, is_unsafe: true/false, project_slug: 'W'})
# Let's just do a generic replacement for Symbol inserts.
def repl_symbol(m):
    props_str = m.group(1)
    # Extract properties
    props = {}
    for kv in props_str.split(','):
        if ':' in kv:
            k, v = kv.split(':', 1)
            props[k.strip()] = v.strip().strip("'")
            
    id_val = props.get('id', '')
    name_val = props.get('name', '')
    kind_val = props.get('kind', 'function')
    tested_val = props.get('tested', 'false')
    is_public = props.get('is_public', 'true')
    is_nif = props.get('is_nif', 'false')
    is_unsafe = props.get('is_unsafe', 'false')
    project_slug = props.get('project_slug', 'global')
    
    return f"INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('{id_val}', '{name_val}', '{kind_val}', {tested_val}, {is_public}, {is_nif}, '{project_slug}')"

content = re.sub(r"MERGE \([^:]*:Symbol \{([^}]+)\}\)", repl_symbol, content)

# 4. MATCH ... MERGE (a)-[:REL]->(b)
def repl_edge(m):
    a_label = m.group(1)
    a_props = m.group(2)
    b_label = m.group(3)
    b_props = m.group(4)
    rel_type = m.group(5)
    
    # We need to extract the IDs. In tests, we often use path for File and name or id for Symbol.
    a_id = ''
    if 'path:' in a_props:
        a_id = re.search(r"path:\s*'([^']+)'", a_props).group(1)
    elif 'id:' in a_props:
        a_id = re.search(r"id:\s*'([^']+)'", a_props).group(1)
    elif 'name:' in a_props:
        # Fallback to global::name for tests
        name = re.search(r"name:\s*'([^']+)'", a_props).group(1)
        a_id = f"global::{name}"
        
    b_id = ''
    if 'path:' in b_props:
        b_id = re.search(r"path:\s*'([^']+)'", b_props).group(1)
    elif 'id:' in b_props:
        b_id = re.search(r"id:\s*'([^']+)'", b_props).group(1)
    elif 'name:' in b_props:
        name = re.search(r"name:\s*'([^']+)'", b_props).group(1)
        b_id = f"global::{name}"
        
    return f"INSERT INTO {rel_type} (source_id, target_id) VALUES ('{a_id}', '{b_id}')"

content = re.sub(r"MATCH \([a-z]+:([A-Za-z]+) \{([^}]+)\}\), \([a-z]+:([A-Za-z]+) \{([^}]+)\}\) MERGE \([a-z]+\)-\[:([A-Z_]+)\]->\([a-z]+\)", repl_edge, content)


with open('src/axon-core/src/mcp.rs', 'w') as f:
    f.write(content)

