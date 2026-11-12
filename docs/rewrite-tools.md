# Rewrite tools

The composer's Rewrite runs a coding CLI that is already installed on the Mac (`crates/app/src/rewrite.rs`). The CLI does one job: it reads the draft, applies the system prompt, and returns text. Anything else it loads costs time, can derail the answer, and puts the user's private setup into a chat message.

## Rule

A tool is offered only when one headless call can run with none of these:

- Instruction files: global or project `AGENTS.md`, `CLAUDE.md`, rules folders.
- Skills, or a skills catalog in the prompt.
- Tools of any kind, including MCP helper tools.
- MCP servers, which also slow the start.
- Hooks, plugins, and memory.

Each switch must come from the tool's own flags, documented settings, or environment variables. Relocating its home folder, rewriting `HOME`, or copying its credentials is a workaround, and workarounds follow [workarounds.md](workarounds.md).

## Tools offered

Tests in `rewrite.rs` pin the flags below.

| Tool | How it is isolated | What still reaches it |
| --- | --- | --- |
| Pi | `--no-tools --no-context-files --no-extensions --no-mcp --no-skills --no-prompt-templates --no-session` | Nothing. A probe reported a clean context. |
| Claude Code | `--setting-sources ""` (no user, project, or local settings: no `CLAUDE.md`, hooks, or plugins), `--tools ""`, `--strict-mcp-config`, `--disallowedTools "mcp__*"`, `--disable-slash-commands`, `--system-prompt` | Organization managed policy, by design, and the account email Claude Code attaches. `--bare` would also drop those, but it requires an API key in place of a subscription login. |
| Codex | `--ignore-user-config` (drops the MCP servers in `config.toml`), `--ignore-rules`, `--disable hooks`, `--disable plugins`, `--disable memories`, `--disable skill_search`, `-c skills.include_instructions=false`, `-c project_doc_max_bytes=0`, a read-only sandbox, and the shell and other tool features disabled | The global `$CODEX_HOME/AGENTS.md`. Codex loads it on every run with no switch (`codex-rs/codex-home/src/instructions/mod.rs`). |

## Not offered: Grok (Grok Build CLI)

Grok was removed after its rewrites timed out or came back as "I'll load the voice skill" instead of a rewrite. Its source ([xai-org/grok-build](https://github.com/xai-org/grok-build), read at `2bdd1d6`) shows why. A headless run with `--system-prompt-override` still gets:

- **Instruction files.** `$GROK_HOME/AGENTS.md` and the project instruction files are inserted as a user message on the first prompt (`xai-grok-shell/src/session/acp_session_impl/session_setup.rs`). The project file names have no off switch (`xai-grok-config/src/compat.rs`).
- **The skills catalog.** It arrives as a system reminder whatever tools are allowed. `~/.agents/skills` is always scanned (`xai-grok-agent/src/prompt/skills.rs`). The model loads a skill by reading its `SKILL.md`.
- **MCP servers.** They start with the session, whatever `--max-turns` or `--tools` say, and the first turn waits up to 2 seconds for them (`run_loop.rs`, `mcp_snapshot.rs`). There is no global off switch.
- **Hooks and plugins.** Hooks in `$GROK_HOME` and the project's `.grok/hooks` always load. Plugins under `~/.claude/plugins` have no switch.
- **No real tool removal.** `--tools ""` applies no restriction, and the MCP helper tools `search_tool` and `use_tool` survive any allowlist (`xai-grok-agent/src/tool_list.rs`).

Getting close would take an agent definition file (`agentsMd: false`, `discoverSkills: false`), a temporary `GROK_HOME` with `GROK_AUTH_PATH` pointed at the real login, ten `GROK_*_ENABLED=0` variables, `--no-memory`, and a denylist naming every tool. Plugins and some hooks would still load unless `HOME` itself moved. That is a workaround stack, not a supported mode.

Add Grok back when Grok Build ships a single switch for a bare headless run, with no instruction files, skills, MCP servers, hooks, plugins, memory, or tools, comparable to Pi's `--no-*` flags or Claude Code's `--bare`. Check its CLI help and the source above before re-adding it, and update this file.
