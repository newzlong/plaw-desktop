---
name: skill-creator
description: Create new skills, modify and improve existing skills. Use when users want to create a skill from scratch, update or optimize an existing skill, or turn a workflow into a reusable skill. Trigger whenever users mention "create a skill", "make a skill", "turn this into a skill", "new skill", or want to capture a repeatable workflow.
compatibility: verified
---

# Skill Creator

A skill for creating new skills and iteratively improving them in Plaw Desktop.

## Core Workflow

1. Understand what the user wants the skill to do
2. Write a draft SKILL.md
3. Test it with the user (try a few prompts, see if the skill triggers and works)
4. Improve based on feedback
5. Repeat until the user is happy

Your job is to figure out where the user is in this process and help them move forward. Maybe they say "I want a skill for X" — help them define it, draft it, and test it. Maybe they already have a draft — go straight to testing and improving.

Be flexible. If the user says "just vibe with me", skip the formal process and iterate casually.

## Communicating with the User

Plaw targets a wide range of users — from developers to people who just installed their first app. Pay attention to context cues:

- Use plain language by default
- "evaluation" and "test" are fine; "assertion" and "JSON schema" need context cues that the user is technical
- Briefly explain terms if you're unsure the user will understand

---

## Creating a Skill

### Step 1: Capture Intent

Start by understanding what the user wants. The conversation might already contain a workflow to capture (e.g., "turn this into a skill"). If so, extract from history first.

Key questions:
1. What should this skill do?
2. When should it trigger? (what phrases/contexts)
3. What's the expected output?

### Step 2: Interview and Research

Ask about edge cases, input/output formats, success criteria, and dependencies. Use `web_fetch` or `search` if you need to look up best practices.

### Step 3: Write the SKILL.md

Create the skill directory and SKILL.md:

```
skill-name/
├── SKILL.md (required)
│   ├── YAML frontmatter (name, description, compatibility)
│   └── Markdown instructions
└── Bundled Resources (optional)
    ├── scripts/    - Helper scripts for repetitive tasks
    ├── references/ - Docs loaded as needed
    └── assets/     - Templates, icons, etc.
```

#### Frontmatter Fields

```yaml
---
name: my-skill
description: What it does and when to trigger. Be specific and slightly "pushy" — list concrete scenarios so the AI knows when to activate this skill.
compatibility: verified
---
```

- **name**: Skill identifier (use kebab-case, e.g., `my-cool-skill`)
- **description**: Primary trigger mechanism. Include both what the skill does AND when to use it. Be generous with trigger scenarios — undertriggering is more common than overtriggering.
- **compatibility**: See Compatibility Audit section below

#### Writing Guidelines

- Keep SKILL.md under 500 lines (in compact mode only name+description are injected)
- Use imperative form ("Do X", not "You should do X")
- Explain the **why** behind instructions — models respond better to reasoning than rigid MUSTs
- Include examples where helpful
- For multi-domain skills, organize by variant with separate reference files

**Example pattern:**
```markdown
## Commit message format
**Example 1:**
Input: Added user authentication with JWT tokens
Output: feat(auth): implement JWT-based authentication
```

#### Progressive Disclosure

Skills load in three levels:
1. **Metadata** (name + description) — always in context (~100 words)
2. **SKILL.md body** — loaded when skill triggers (<500 lines ideal)
3. **Bundled resources** — read on demand (unlimited size)

Put the most critical instructions in the body. Reference files for detailed docs.

#### Security

Skills must not contain malware, exploit code, or anything that could compromise security. A skill's contents should not surprise the user if described. Do not create skills designed for unauthorized access or data exfiltration.

### Step 4: Compatibility Audit (Required)

After writing the SKILL.md, evaluate and add a `compatibility:` tag:

- **verified** — Only uses built-in tools (shell, read_file, write_file, edit_file, list_dir, search, web_fetch, http_request, memory_read, memory_write). No external API keys, no extra software. Safe to run immediately.
- **needs-setup** — Requires external API keys (GitHub token, etc.), specific software (Docker, ffmpeg, Chrome), or external services. Works after user configures dependencies.
- **incompatible** — Fundamentally conflicts with Plaw architecture: requires MCP servers, assumes VS Code/Electron host, depends on nonexistent tools, or modifies system config.

Architecture rules to check:
1. Tool names must match Plaw's built-in set
2. No MCP server dependencies
3. Shell commands must work on Windows (PowerShell) or use cross-platform alternatives
4. No assumptions about IDE or editor environment
5. File paths should be relative, not absolute
6. Storage uses memory_read/memory_write (SQLite), not filesystem-based memory
7. No .sh shell scripts (Plaw audit blocks them)

If the skill is `incompatible`, warn the user and explain what's wrong and whether it can be fixed.

### Step 5: Test the Skill

Come up with 2-3 realistic test prompts — the kind of thing a real user would actually say. Share them with the user for confirmation.

Then test each prompt by reading the skill's SKILL.md and following its instructions to complete the task. Present the results to the user and ask for feedback.

### Step 6: Iterate

Based on user feedback:

1. **Generalize** — don't overfit to test examples. The skill will be used across many different prompts.
2. **Keep it lean** — remove instructions that aren't pulling their weight.
3. **Explain the why** — reasoning > rigid rules. If you find yourself writing ALWAYS/NEVER in caps, try explaining the reasoning instead.
4. **Bundle repeated work** — if every test run produces the same helper script, bundle it in `scripts/`.

Repeat testing and improving until:
- The user is happy
- The feedback is all positive
- You're not making meaningful progress

---

## Improving an Existing Skill

When the user wants to modify an existing skill:

1. Read the current SKILL.md
2. Understand what needs to change
3. Make the edits
4. Re-evaluate the compatibility tag (it may change)
5. Test with a few prompts if the change is significant
6. Ask for user feedback

---

## Description Optimization Tips

The description field is the primary mechanism for skill triggering. Good descriptions:

- State what the skill does clearly
- List specific trigger scenarios and phrases
- Are slightly "pushy" — better to overtrigger than undertrigger
- Include adjacent use cases the user might not think to phrase exactly

Example — instead of:
> "Build dashboards for data visualization."

Write:
> "Build dashboards and data visualizations. Use this skill whenever the user mentions dashboards, charts, graphs, metrics display, data visualization, internal analytics, or wants to display any kind of data visually, even if they don't explicitly say 'dashboard'."

---

## Skill Installation Path

After creating the skill, it is automatically available in `skills/` directory. The user needs to start a new conversation for the skill to take effect (skills are loaded at conversation start).

Remind the user of this after creation is complete.
