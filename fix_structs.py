import os
import glob
import re

for filepath in glob.glob("src/axon-core/src/parser/*.rs"):
    if filepath.endswith("mod.rs"): continue
    with open(filepath, "r") as f:
        content = f.read()
    
    # We only want to modify the files where Symbol is constructed
    content = re.sub(r'(Symbol\s*\{[^\}]+docstring:\s*[^,\}]+)(,?)(\s*\})', r'\1,\n                        is_entry_point: false,\n                        properties: std::collections::HashMap::new(),\3', content)
    content = re.sub(r'(Relation\s*\{[^\}]+rel_type:\s*[^,\}]+)(,?)(\s*\})', r'\1,\n                                        properties: std::collections::HashMap::new(),\3', content)
    
    with open(filepath, "w") as f:
        f.write(content)
