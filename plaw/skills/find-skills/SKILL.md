---
name: find-skills
description: Helps users discover and install agent skills when they ask questions like "how do I do X", "find a skill for X", "is there a skill that can...", or express interest in extending capabilities. This skill should be used when the user is looking for functionality that might exist as an installable skill.
---

# Find Skills

This skill helps you discover and install skills from ClawHub, the public skill registry.

## When to Use This Skill

Use this skill when the user:

- Asks "how do I do X" where X might be a common task with an existing skill
- Says "find a skill for X" or "is there a skill for X"
- Asks "can you do X" where X is a specialized capability
- Expresses interest in extending agent capabilities
- Wants to search for tools, templates, or workflows
- Mentions they wish they had help with a specific domain (design, testing, deployment, etc.)

## Available Commands

Plaw has built-in ClawHub integration. Use these shell commands:

- `plaw skills search "<query>"` - Search for skills on ClawHub
- `plaw skills install <slug>` - Install a skill from ClawHub by its slug
- `plaw skills list` - List all installed skills
- `plaw skills remove <name>` - Remove an installed skill

**Browse skills at:** https://clawhub.ai

## How to Help Users Find Skills

### Step 1: Understand What They Need

When a user asks for help with something, identify:

1. The domain (e.g., React, testing, design, deployment)
2. The specific task (e.g., writing tests, creating animations, reviewing PRs)
3. Whether this is a common enough task that a skill likely exists

### Step 2: Search for Skills

Run the search command with a relevant query:

```bash
plaw skills search "react performance"
```

For example:

- User asks "how do I make my React app faster?" -> `plaw skills search "react performance"`
- User asks "can you help me with PR reviews?" -> `plaw skills search "pr review"`
- User asks "I need to create a changelog" -> `plaw skills search "changelog"`

### Step 3: Present Options to the User

When you find relevant skills, present them to the user with:

1. The skill name and what it does
2. The install command they can run
3. A link to learn more at clawhub.ai

Example response:

```
I found a skill that might help! The "react-best-practices" skill provides
React and Next.js performance optimization guidelines.

To install it:
plaw skills install react-best-practices

Learn more: https://clawhub.ai
```

### Step 4: Offer to Install

If the user wants to proceed, install the skill for them:

```bash
plaw skills install <slug>
```

IMPORTANT: After installation, tell the user the skill has been installed successfully, but they need to **start a new conversation** for the skill to take effect (skills are loaded when a conversation begins, not mid-conversation).

## Common Skill Categories

When searching, consider these common categories:

| Category        | Example Queries                          |
| --------------- | ---------------------------------------- |
| Web Development | react, nextjs, typescript, css, tailwind |
| Testing         | testing, jest, playwright, e2e           |
| DevOps          | deploy, docker, kubernetes, ci-cd        |
| Documentation   | docs, readme, changelog, api-docs        |
| Code Quality    | review, lint, refactor, best-practices   |
| Design          | ui, ux, design-system, accessibility     |
| Productivity    | workflow, automation, git                |

## Tips for Effective Searches

1. **Use specific keywords**: "react testing" is better than just "testing"
2. **Try alternative terms**: If "deploy" doesn't work, try "deployment" or "ci-cd"
3. **Search in English**: ClawHub skills are primarily in English

## When No Skills Are Found

If no relevant skills exist:

1. Acknowledge that no existing skill was found
2. Offer to help with the task directly using your general capabilities
3. Suggest the user could create their own skill

Example:

```
I searched for skills related to "xyz" but didn't find any matches.
I can still help you with this task directly! Would you like me to proceed?
```
