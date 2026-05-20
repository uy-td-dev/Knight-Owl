+++
schema_version = 1
id          = "grep_callsites"
name        = "Grep Callsites"
description = "Find all references to a symbol across the codebase"
trigger     = "when asked 'where is X used' or 'callers of Y'"
recommended_tools = ["grep", "read_file"]
+++

When this skill activates:
1. Run `grep -rn "<symbol>" --include="*.rs"` first.
2. Group hits by file.
3. For each group, `read_file` ±5 lines around hit.
4. Output: markdown table {file, line, surrounding code}.

Edge cases:
- Skip vendored / target / node_modules.
- If symbol is too generic (e.g. `Result`), narrow scope first.
