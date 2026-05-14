# Vulcan Agent Prompts

Reusable prompt files for AI agents using the Vulcan MCP server to trade on Phoenix Perpetuals DEX.

## Usage

These prompts are compatibility fallbacks. Prefer MCP resources and bundled skills when the client supports them:

- **`system.md`** — Minimal fallback prompt for agents that cannot read MCP resources or installed skills. Do not include it for clients that can read `vulcan://context`.

## Document Ownership

- `../CONTEXT.md` / `vulcan://context` — Canonical runtime contract for all agents.
- `../skills/vulcan/SKILL.md` — Single entry skill for skill-capable agents (safety rules, router, preflight gate).
- `../skills/*/SKILL.md` — Focused workflows.
- `system.md` — Fallback prompt for non-resource, non-skill agents.
- `../agents/tool-catalog.json` and `../agents/error-catalog.json` — Machine-readable schemas and error details.

When these files disagree, treat `CONTEXT.md` and the machine-readable catalogs as authoritative.

## Agent Skills

Skills are the canonical home for task-specific workflows. Prefer installing the bundled `agentskills.io`-style skills when your client supports them:

```bash
vulcan agent install --target cursor --scope user
vulcan agent install --target claude --scope project
vulcan agent doctor --target cursor --scope user
```

The broad entry point is `skills/vulcan/SKILL.md`; task-specific skills live under `skills/*/SKILL.md`. MCP clients should use `vulcan://skills/index` to discover the same content.

MCP clients can also read:

- `vulcan://context`
- `vulcan://skills/index`
- `vulcan://agents/tool-catalog`
- `vulcan://agents/error-catalog`

## With Claude Code

Add to your MCP config and reference these files in your system prompt or CLAUDE.md.

## With Other Agents

If the client cannot read MCP resources or installed skills, include `system.md` content in the agent's system prompt. Otherwise load `vulcan://context` plus the relevant skills.
