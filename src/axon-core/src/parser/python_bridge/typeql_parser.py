import sys
import json
import re

def parse_typeql(content):
    symbols = []
    relations = []
    
    # Very basic regex parser for ontology extraction since typedb-driver native python library requires shared objects not always present
    # Extract entities
    for match in re.finditer(r'([a-zA-Z0-9_-]+)\s+sub\s+entity\b', content, re.IGNORECASE):
        name = match.group(1)
        symbols.append({
            "name": name,
            "kind": "entity_type",
            "start_line": 1,
            "end_line": 1,
            "is_entry_point": False,
            "is_public": True,
            "properties": {}
        })

    # Extract relations
    for match in re.finditer(r'([a-zA-Z0-9_-]+)\s+sub\s+relation\b', content, re.IGNORECASE):
        name = match.group(1)
        symbols.append({
            "name": name,
            "kind": "relation_type",
            "start_line": 1,
            "end_line": 1,
            "is_entry_point": False,
            "is_public": True,
            "properties": {}
        })

    # Extract rules
    for match in re.finditer(r'([a-zA-Z0-9_-]+):\s*rule\s+when\s*\{', content, re.IGNORECASE):
        name = match.group(1)
        symbols.append({
            "name": name,
            "kind": "rule",
            "start_line": 1,
            "end_line": 1,
            "is_entry_point": False,
            "is_public": True,
            "properties": {}
        })

    # Extract ownerships (owns) and map them to the previous block
    blocks = re.split(r';\s*$', content, flags=re.MULTILINE)
    for block in blocks:
        if 'sub entity' in block or 'sub relation' in block:
            lines = block.split('\n')
            current_entity = None
            for line in lines:
                if 'sub entity' in line or 'sub relation' in line:
                    match = re.search(r'([a-zA-Z0-9_-]+)\s+sub', line)
                    if match:
                        current_entity = match.group(1)
                elif current_entity and 'owns' in line:
                    match = re.search(r'owns\s+([a-zA-Z0-9_-]+)', line)
                    if match:
                        attr = match.group(1)
                        symbols.append({
                            "name": attr,
                            "kind": "attribute",
                            "start_line": 1,
                            "end_line": 1,
                            "is_entry_point": False,
                            "is_public": True,
                            "properties": {}
                        })
                        relations.append({
                            "from": current_entity,
                            "to": attr,
                            "rel_type": "owns",
                            "properties": {}
                        })

    return {"symbols": symbols, "relations": relations}

if __name__ == "__main__":
    if len(sys.argv) > 1:
        with open(sys.argv[1], 'r') as f:
            content = f.read()
            print(json.dumps(parse_typeql(content)))
