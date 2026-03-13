import os
import re

dir_path = "/home/dstadel/projects/axon/src/axon-core/src/parser"

# We want to match `properties: <something>,` or `properties,`
# that is immediately followed by `}` on the next line (with some whitespace).
# In Rust, the last field might not have a comma, but in `axon` it seems they all have it or are followed by `}`.
# Let's do a more robust approach:
# Look for `Symbol {` and then find the corresponding `}`.
# Before the `}`, insert `embedding: None,`.

for root, dirs, files in os.walk(dir_path):
    for file in files:
        if file.endswith(".rs") and file != "mod.rs":
            file_path = os.path.join(root, file)
            with open(file_path, "r") as f:
                content = f.read()

            new_content = ""
            i = 0
            while i < len(content):
                # Look for "Symbol {"
                idx = content.find("Symbol {", i)
                if idx == -1:
                    new_content += content[i:]
                    break
                
                # Find the end of this Symbol instantiation
                # We can just count braces
                brace_count = 0
                j = idx + 6
                in_string = False
                escape = False
                end_idx = -1
                for k in range(j, len(content)):
                    if escape:
                        escape = False
                        continue
                    if content[k] == '\\':
                        escape = True
                        continue
                    if content[k] == '"':
                        in_string = not in_string
                        continue
                    if not in_string:
                        if content[k] == '{':
                            brace_count += 1
                        elif content[k] == '}':
                            brace_count -= 1
                            if brace_count == 0:
                                end_idx = k
                                break
                
                if end_idx != -1:
                    # We found the block from idx to end_idx
                    # Find the last line before `}`
                    # To add `embedding: None,` with the correct indentation
                    symbol_block = content[idx:end_idx]
                    
                    # find indentation of `}`
                    last_newline = content.rfind('\n', idx, end_idx)
                    indent = ""
                    if last_newline != -1:
                        indent_len = end_idx - last_newline - 1
                        indent = " " * indent_len
                    
                    # We need to ensure there is a comma after the last field
                    # The last non-whitespace character before `\n` or `}` should be `,`
                    
                    block_stripped = symbol_block.rstrip()
                    if block_stripped and block_stripped[-1] != ',':
                        # We need to add a comma
                        symbol_block = symbol_block.rstrip() + ",\n"
                    elif not symbol_block.endswith('\n'):
                        symbol_block += "\n"
                        
                    # Now add `embedding: None,`
                    new_symbol_block = symbol_block + indent + "    embedding: None,\n" + indent
                    
                    new_content += content[i:idx] + new_symbol_block + "}"
                    i = end_idx + 1
                else:
                    new_content += content[i:idx+8]
                    i = idx + 8
                    
            if content != new_content:
                with open(file_path, "w") as f:
                    f.write(new_content)
                print(f"Updated {file}")
