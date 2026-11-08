
TOOLS FOR THIS RUN:

- Shell commands are denied in this headless mode and a single denied `run_command` call aborts the whole run with no answer. Never call `run_command`; use only your file viewing, directory listing, file search and grep tools.
- Read-only MCP tools are available through `call_mcp_tool` and are preferred over grep for symbol and reference questions: blast-radius `open_project`, `find_symbol`, `symbol_info`, `blast_radius`, `diff_blast`, `call_slice`, `module_graph`, `dead_code`, and codegraph `codegraph_explore`. Any other tool request (other MCP tools, other servers, commands) is denied and aborts the run.
