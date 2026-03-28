import re
import sys

def convert():
    try:
        with open("src/axon-core/src/mcp.rs", "r") as f:
            content = f.read()
    except Exception as e:
        print(e)
        return

    symbol_name_to_id = {}
    symbol_id_to_id = {}

    def extract_props(props_str):
        props = {}
        for kv in re.finditer(r'([a-zA-Z_]+):\s*([^,}]+)', props_str):
            k = kv.group(1).strip()
            v = kv.group(2).strip()
            props[k] = v
        return props

    def filter_columns(props, table_name):
        # Only allow columns that actually exist in the schema
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
        if 'id' in props:
            symbol_id_to_id[props['id'].strip("'")] = sym_id.strip("'")
            
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

    # First pass: restore to MERGE where I replaced previously, or just work on original if I run `git checkout`
    pass

if __name__ == "__main__":
    convert()
