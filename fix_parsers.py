import os, glob, re

for filepath in glob.glob("src/axon-core/src/parser/*.rs"):
    with open(filepath, "r") as f:
        content = f.read()
    
    # We want to change:
    # let tree = match parse_with_wasm_safe("...", self.wasm_bytes, content) {
    #     Some(t) => t,
    #     None => return ExtractionResult { symbols: Vec::new(), relations: Vec::new(), error_reason: None },
    # };
    # TO
    # let tree = match parse_with_wasm_safe("...", self.wasm_bytes, content) {
    #     Ok(t) => t,
    #     Err(e) => return ExtractionResult { symbols: Vec::new(), relations: Vec::new(), error_reason: Some(e) },
    # };
    
    # Also handle the typescript parser where it sometimes has `, }` or `;`
    
    new_content = re.sub(
        r"Some\(t\)\s*=>\s*t,\s*None\s*=>\s*return\s*ExtractionResult\s*\{\s*symbols:\s*Vec::new\(\),\s*relations:\s*Vec::new\(\),\s*error_reason:\s*None,?\s*\}",
        "Ok(t) => t,\n            Err(e) => return ExtractionResult { symbols: Vec::new(), relations: Vec::new(), error_reason: Some(e) }",
        content
    )
    
    # Same for go/python where there might be a trailing comma
    
    if content != new_content:
        with open(filepath, "w") as f:
            f.write(new_content)
        print(f"Updated {filepath}")
