
CHECKLIST FOR THIS REVIEWER (you have no repository skills configured, so follow this explicitly):

1. Read AGENTS.md and CLAUDE.md in the working directory first.
2. Check every changed line of the diff against the style rules above (blank lines after blocks, variables at the top, no inline comments, doc comments, item order, `.clone()` smells).
3. Verify each claim by opening the file and the line before you report it.
4. No speculative findings: if you cannot show the failure scenario in the actual code, leave it out.
5. Shell commands are denied in this headless mode and a single denied `run_command` call aborts the whole run with no answer. Never call `run_command`; use only your file viewing, directory listing, file search and grep tools.
6. Read-only MCP tools are available through `call_mcp_tool` and are preferred over grep for symbol and reference questions: blast-radius `open_project`, `find_symbol`, `symbol_info`, `blast_radius`, `diff_blast`, `call_slice`, `module_graph`, `dead_code`, and codegraph `codegraph_explore`. Any other tool request (other MCP tools, other servers, commands) is denied and aborts the run.
