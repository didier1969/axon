import re

def convert():
    try:
        with open("src/axon-core/src/mcp.rs", "r") as f:
            content = f.read()
    except Exception as e:
        print(e)
        return

    symbol_name_to_id = {}

    def extract_props(props_str):
        props = {}
        for kv in re.finditer(r'([a-zA-Z_]+):\s*([^,}]+)', props_str):
            k = kv.group(1).strip()
            v = kv.group(2).strip()
            props[k] = v
        return props

    def filter_columns(props, table_name):
        if table_name == "Symbol":
            valid_cols = {'id', 'name', 'kind', 'tested', 'is_public', 'is_nif'}
        elif table_name == "File":
            valid_cols = {'path', 'project_slug', 'status', 'size', 'priority', 'mtime', 'worker_id'}
        else:
            valid_cols = set(props.keys())
            
        filtered_cols = []
        filtered_vals = []
        for k, v in props.items():
            if k in valid_cols:
                filtered_cols.append(k)
                filtered_vals.append(v)
        return filtered_cols, filtered_vals

    def replace_symbol(m):
        prefix = m.group(1)
        props_str = m.group(2)
        suffix = m.group(3)
        props = extract_props(props_str)
        
        sym_id = props.get('id', "'unknown'")
        if 'name' in props:
            symbol_name_to_id[props['name'].strip("'")] = sym_id.strip("'")
            
        cols, vals = filter_columns(props, "Symbol")
        
        return f'{prefix}INSERT INTO Symbol ({", ".join(cols)}) VALUES ({", ".join(vals)}){suffix}'

    def replace_format_symbol(m):
        prefix = m.group(1)
        props_str = m.group(2)
        suffix = m.group(3)
        props_str = props_str.replace('{{', '').replace('}}', '')
        props = extract_props(props_str)
        
        cols, vals = filter_columns(props, "Symbol")
        
        return f'{prefix}INSERT INTO Symbol ({", ".join(cols)}) VALUES ({", ".join(vals)}){suffix}'

    def replace_file(m):
        prefix = m.group(1)
        props_str = m.group(2)
        suffix = m.group(3)
        props = extract_props(props_str)
        
        if 'project_slug' not in props:
            props['project_slug'] = "'global'"
            
        cols, vals = filter_columns(props, "File")
        
        return f'{prefix}INSERT INTO File ({", ".join(cols)}) VALUES ({", ".join(vals)}){suffix}'

    content = re.sub(
        r'(server\.graph_store\.execute\(")MERGE \([^:]+:Symbol \{(.*?)\}\)("\))',
        replace_symbol,
        content
    )
    
    content = re.sub(
        r'(server\.graph_store\.execute\(&format!\(")MERGE \([^:]+:Symbol \{\{(.*?)\}\}\)("\))',
        replace_format_symbol,
        content
    )

    content = re.sub(
        r'(server\.graph_store\.execute\(")MERGE \([^:]+:File \{(.*?)\}\)("\))',
        replace_file,
        content
    )

    def replace_rel(m):
        prefix = m.group(1)
        match_part = m.group(2)
        rel_type = m.group(3)
        suffix = m.group(4)
        
        nodes = re.findall(r'\([^:]+:([^ ]+) \{(.*?)\}\)', match_part)
        
        if len(nodes) != 2:
            return m.group(0)
            
        source_type, source_props_str = nodes[0]
        target_type, target_props_str = nodes[1]
        
        source_props = extract_props(source_props_str)
        target_props = extract_props(target_props_str)
        
        def get_id(ntype, nprops):
            if ntype == 'File':
                return nprops.get('path', "''")
            elif ntype == 'Symbol':
                if 'id' in nprops:
                    return nprops['id']
                elif 'name' in nprops:
                    name_val = nprops['name'].strip("'")
                    if name_val in symbol_name_to_id:
                        return f"'{symbol_name_to_id[name_val]}'"
                    return nprops['name']
            return "''"
            
        source_id = get_id(source_type, source_props)
        target_id = get_id(target_type, target_props)
        
        return f'{prefix}INSERT INTO {rel_type} (source_id, target_id) VALUES ({source_id}, {target_id}){suffix}'

    content = re.sub(
        r'(server\.graph_store\.execute\(")MATCH (.*?) MERGE \([^)]+\)-\[:([A-Z_]+)\]->\([^)]+\)("\))',
        replace_rel,
        content
    )
    
    def replace_format_rel(m):
        prefix = m.group(1)
        match_part = m.group(2)
        rel_type = m.group(3)
        suffix = m.group(4)
        
        nodes = re.findall(r'\([^:]+:([^ ]+) \{\{(.*?)\}\}\)', match_part)
        if len(nodes) != 2:
            return m.group(0)
            
        source_type, source_props_str = nodes[0]
        target_type, target_props_str = nodes[1]
        
        source_props = extract_props(source_props_str)
        target_props = extract_props(target_props_str)
        
        source_id = source_props.get('id', "''")
        target_id = target_props.get('id', "''")
        
        return f'{prefix}INSERT INTO {rel_type} (source_id, target_id) VALUES ({source_id}, {target_id}){suffix}'

    content = re.sub(
        r'(server\.graph_store\.execute\(&format!\(")MATCH (.*?) MERGE \([^)]+\)-\[:([A-Z_]+)\]->\([^)]+\)("\))',
        replace_format_rel,
        content
    )

    with open("src/axon-core/src/mcp.rs", "w") as f:
        f.write(content)
    
    print("Done")

if __name__ == "__main__":
    convert()
