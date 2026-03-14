import sys
import json
import re

def parse_datalog(content):
    symbols = []
    relations = []
    
    # Very basic regex parser for datalog (.decl and rules)
    # Extract declarations
    for match in re.finditer(r'\.decl\s+([a-zA-Z0-9_-]+)\s*\(', content, re.IGNORECASE):
        name = match.group(1)
        symbols.append({
            "name": name,
            "kind": "datalog_relation",
            "start_line": 1,
            "end_line": 1,
            "is_entry_point": False,
            "is_public": True,
            "properties": {}
        })

    # Extract rules
    for match in re.finditer(r'^([a-zA-Z0-9_-]+)\s*\([^)]*\)\s*:-', content, re.MULTILINE):
        name = match.group(1)
        # Avoid duplicate rule symbols if multiple clauses
        if not any(s["name"] == name and s["kind"] == "datalog_rule" for s in symbols):
            symbols.append({
                "name": name,
                "kind": "datalog_rule",
                "start_line": 1,
                "end_line": 1,
                "is_entry_point": False,
                "is_public": True,
                "properties": {}
            })
    
    lines = content.split('\n')
    for line in lines:
        if ':-' in line:
            parts = line.split(':-')
            head_match = re.search(r'([a-zA-Z0-9_-]+)\s*\(', parts[0])
            if head_match:
                head = head_match.group(1)
                
                # find body atoms
                bodies = re.finditer(r'([a-zA-Z0-9_-]+)\s*\(', parts[1])
                for body in bodies:
                    body_rel = body.group(1)
                    relations.append({
                        "from": head,
                        "to": body_rel,
                        "rel_type": "depends_on",
                        "properties": {}
                    })

    return {"symbols": symbols, "relations": relations}

if __name__ == "__main__":
    if len(sys.argv) > 1:
        with open(sys.argv[1], 'r') as f:
            content = f.read()
            print(json.dumps(parse_datalog(content)))
