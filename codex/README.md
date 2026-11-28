# Codex plugin surface (marketplace)

This folder is the Codex-specific packaging for total-recall. The repo
itself is a multi-harness tool (CLI + MCP; harnesses: opencode, vibe,
codex, pi) — this folder exists only to make total-recall installable as
a Codex plugin from a local marketplace. It says nothing about how the
tool is installed elsewhere.

Contents:

- `plugin.json` — the portable Agent Plugins manifest for the plugin.
- `marketplace.json` — the marketplace catalog that serves the plugin.

## Install from this local marketplace

The `total-recall` binary must be on `PATH` (`cargo install --path .` or
the release binary).

```bash
codex plugin marketplace add /path/to/total-recall/codex
codex plugin list --marketplace total-recall
codex plugin add total-recall@total-recall
```

Uninstall:

```bash
codex plugin remove total-recall@total-recall
```

## Repo-scoped install

When a project already has a `total-recall` checkout, add to the project's
`.codex/config.toml`:

```toml
[plugins."total-recall@total-recall"]
enabled = true
```

## Notes

- The MCP server invoked by the plugin uses `total-recall mcp` with the
  harness set for the Codex store. Generic MCP / CLI install instructions
  for every harness live in the repo's docs, not here.
- The plugin manifest's `onboardingSkill` references the repo's
  `skills/total-recall/SKILL.md` tree. A fully self-contained publish
  would need to bundle that skill; for a local (repo) marketplace the
  reference is relative to the repo root.
- Keep `plugin.json`'s `version` aligned with the release version in
  `Cargo.toml` when tagging.
