import os

path = '/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs'
with open(path, 'r') as f:
    content = f.read()

content = content.replace(
    "\"project_name\": \"BookingSystem\",\n                \"project_slug\": \"BKS\",",
    "\"project_slug\": \"BookingSystem\",\n                \"project_code\": \"BKS\","
)

with open(path, 'w') as f:
    f.write(content)
