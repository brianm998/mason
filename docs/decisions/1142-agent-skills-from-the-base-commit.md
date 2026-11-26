# 1142: agent skills come from the base commit, and nothing else of the worktree's

## Context

Prompt packs sit in the prompt in full on every run and are capped at 32 KB. Agent Skills load
progressively: a session holds each skill's name and description and reads a body only when it
applies (#1140). Agents had no `Skill` tool.

## How the installed CLI loads skills

Checked on Claude Code 2.1.330, from `claude --help`, from strings in the binary, and by running
it with a temporary `HOME` against a scripted API on 127.0.0.1 (no request left the machine):

- `--plugin-dir <dir>` loads a plugin for one session. With `.claude-plugin/plugin.json` naming it
  `p`, each `skills/<name>/SKILL.md` is listed in the init event's `skills` as `p:<name>`, and
  `Skill` calls it under `--permission-mode dontAsk` with `--allowedTools Skill`.
- The working directory's `.claude/skills/` and `.claude/commands/` load too. A directory there
  named `p:<name>` **replaces** the plugin's skill of that name.
- `skillOverrides: {"<name>": "off"}` in `--settings` hides such a skill and refuses it, and the
  plugin's skill of the same name then loads. It does not turn off a plugin's own skill.
- A user skill wins over a project skill of the same name.
- `--setting-sources` is the only switch for a whole source. Leaving out `project` also drops the
  repository's `CLAUDE.md`; leaving out `user` drops the owner's settings and instructions.
  `strictPluginOnlyCustomization` had no effect from `--settings`.
- `disableSkillShellExecution` stops a skill's `` !`command` `` lines. `disableBundledSkills`
  leaves `doctor` and `plugin-authoring` on.
- A `PreToolUse` hook with matcher `Skill` sees `tool_input.skill` and can deny the call.

## Decision

- A project's skills are `.mason/skills/<name>/` on the base commit, each with a `SKILL.md`.
  `<name>` is lowercase letters, digits, `-` or `_`, at most 64. Symbolic links, files over 1 MiB
  and whatever passes 8 MiB in all are left out, with a warning.
- For every Claude session the daemon reads them from the base commit and the adapter writes them
  into the session's own directory in the daemon's state, as a plugin named `mason`, passed with
  `--plugin-dir`. A project with none gets no `--plugin-dir`. The session sweep removes it.
- `Skill` is among the built-in tools of every step kind.
- The worktree's skills and commands are turned off by name (`skillOverrides`), except a name the
  owner also has, whose copy wins. A `Skill` guard (`mason hook skill`, with the session's
  `skills.json`) refuses anything but the project's skills and the owner's, which covers what is
  written after the launch. A guard that cannot run refuses.
- **The owner's user-level skills load by default** (decision 0003 keeps user settings on):
  their skills, commands and enabled plugins. `[agent] user_skills = false` in the project file
  turns their skills and commands off by name, turns their plugins off (`enabledPlugins`) and
  makes the guard refuse them. Their settings and instructions still apply.
- The CLI's bundled skills are off, and so is shell execution from any skill, the owner's
  included: it runs outside the command guard, in read-only steps too.
- `mason task show` lists the skills each step run called, from the stream's `Skill` calls,
  refused ones included.

## Consequences

- `.mason/skills/**` is under the protected `.mason/**`: on MASON's own repository skills change
  only with the owner's approval, as packs do. A skill's frontmatter (`allowed-tools`, `hooks`)
  is part of what is approved.
- A repository's committed `.claude/skills/` and `.claude/commands/` no longer run in MASON's
  sessions. Move them to `.mason/skills/` to keep them.
- A `.claude/skills/` in a subdirectory is not turned off by name; the guard refuses it.
- Where the owner has a skill from a source not read here (managed settings, claude.ai sync
  outside `~/.claude/skills`), the guard refuses it.
- Like the command guard, this is not a boundary against a develop agent that sets out to rewrite
  the daemon's state directory with Bash. The OS sandbox is the boundary.
- Codex sessions get no skills.
