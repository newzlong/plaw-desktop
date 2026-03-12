---

name: audit-skills
description: "Audit and tag installed skills for compatibility with Plaw Desktop. Use when the user asks to check skills, audit skills, tag skills, review skill compatibility, or says things like 'check my skills', 'audit skills', 'tag skills', 'review skills compatibility', 'which skills work', 'which skills are broken'."
compatibility: verified
risk: warning
---

# Audit Skills

Audit installed skills and tag them with compatibility and risk labels.

## What This Skill Does

1. Scan all installed skills
2. Read each SKILL.md
3. Check against Plaw compatibility rules
4. Write `compatibility` and `risk` tags into each SKILL.md frontmatter

## Compatibility Tags

Assign ONE of these to every skill:

| Tag | Meaning | Criteria |
|-----|---------|----------|
| `verified` | Works out of the box | Only uses built-in tools, no external deps |
| `needs-setup` | Works after user config | Needs API keys, external software (Docker, ffmpeg, etc.) |
| `incompatible` | Cannot work in Plaw | Requires MCP, VS Code, nonexistent tools, or fundamentally broken |

## Risk Tags

Assign ONE of these:

| Tag | Meaning | Criteria |
|-----|---------|----------|
| `safe` | No side effects | Read-only or memory-only operations |
| `warning` | Has side effects | Writes files, runs shell commands, calls external APIs |
| `danger` | High-risk operations | Modifies system config, sends emails, financial operations, deletes data |

## Audit Checklist (7 Rules)

For each skill, check these rules:

### Rule 1: Storage
- PASS: Uses `memory_store`/`memory_recall` or no storage
- FAIL: Uses filesystem-based memory (writes to `memory/`, `MEMORY.md`, etc.)

### Rule 2: No shell scripts
- PASS: No `.sh` files in skill directory
- FAIL: Contains `.sh` files (Plaw audit blocks them)

### Rule 3: Clean directory
- PASS: Skill dir only contains SKILL.md and optional subdirs (scripts/, references/, assets/)
- FAIL: Non-skill dirs like `_adapters/`, `_utils/` under skills/

### Rule 4: Tool names
- PASS: Only references tools that exist in Plaw (see list below)
- FAIL: References MCP tools, VS Code commands, or tools that don't exist

### Rule 5: Agent delegation
- PASS: Uses `delegate`/`subagent_spawn`/`parallel_delegate` or none
- FAIL: Defines custom agent types or MCP-based delegation

### Rule 6: Windows compatible
- PASS: Shell commands work on Windows/PowerShell, or skill is shell-agnostic
- FAIL: Linux-only commands (apt-get, systemctl, etc.) without alternatives

### Rule 7: SKILL.md format
- PASS: Has YAML frontmatter with name + description
- FAIL: Missing frontmatter, missing name, or missing description

## Plaw Built-in Tools (Reference)

These are the tools available in Plaw Desktop:

- `shell` - execute commands (PowerShell on Windows)
- `read_file`, `write_file`, `edit_file`, `list_dir` - file operations
- `search` - search file contents (grep-like)
- `memory_store`, `memory_recall`, `memory_forget` - SQLite memory
- `web_search_tool` - Bing RSS web search
- `web_fetch` - fetch webpage as markdown
- `http_request` - HTTP API calls
- `browser_navigate`, `browser_click`, `browser_screenshot` - browser automation
- `cron_add`, `cron_list`, `cron_remove` - scheduled tasks
- `delegate`, `subagent_spawn`, `parallel_delegate` - agent delegation

Any tool NOT in this list is unavailable.

## Workflow

### Full Audit (all skills)

1. Run `list_dir` on the skills directory to get all skill names
2. Use `parallel_delegate` to audit multiple skills concurrently (batch of 5-8)
3. Each sub-task: read SKILL.md, check 7 rules, determine compatibility + risk
4. Collect results, present summary table to user
5. Ask user for confirmation before writing tags
6. Write tags into each SKILL.md frontmatter via `edit_file`

### Single Skill Audit

1. Read the target SKILL.md
2. Check 7 rules
3. Determine compatibility + risk tags
4. Show result to user
5. Write tags if user approves

## Output Format

Present results as a table:

```
| Skill | Compatibility | Risk | Issues |
|-------|--------------|------|--------|
| weather | verified | safe | - |
| docker-essentials | needs-setup | warning | Requires Docker installed |
| nostr-logging | incompatible | - | Uses nonexistent MCP tools |
```

## Writing Tags

When writing tags to SKILL.md, edit the YAML frontmatter:

If frontmatter exists, add/update fields:
```yaml
---
name: skill-name
description: ...
compatibility: verified
risk: safe
---
```

If no frontmatter, add it:
```yaml
---
name: skill-name
description: (extract from first paragraph)
compatibility: needs-setup
risk: warning
---
```

## Important Notes

- Use `parallel_delegate` for batch audits (much faster than sequential)
- Do NOT modify skill instructions, only add/update frontmatter tags
- If unsure about compatibility, default to `needs-setup`
- If unsure about risk, default to `warning`
- Skip `find-skills` and `skill-creator` (protected skills, already tagged)
- After tagging, tell user to refresh the Skills page in Plaw UI to see updates
