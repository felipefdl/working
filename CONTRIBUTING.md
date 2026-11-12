# Contributing

Working is personal MIT open source maintained by Felipe Lima. Useful patches are welcome on [felipefdl/working](https://github.com/felipefdl/working). By participating, you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Language

American English in code, commits, docs, comments, and PR text.

## Toolchain

- **MSRV:** Rust **1.99.1** (`rust-version` in the workspace `Cargo.toml`)
- Format: `rustfmt.toml` (width 120, 2-space indent, edition 2024)
- Clippy: workspace deny bar (`-D warnings` in the quality gate)

## Quality gate

From the repository root (with [just](https://github.com/casey/just) installed):

```bash
just check
```

That runs format check, clippy, tests, and supply-chain gates. The contract is [docs/rust-quality.md](docs/rust-quality.md). Do not land work that fails format, clippy, or deny/audit/machete.

Mid-change, `just task <package>` or `just t '<nextest filter>'` covers one crate or a test filter. Completion still needs `just check`.

## Conventions

- Conventional commits: `type(scope): subject` (lowercase subject, no trailing period)
- Branches: `type/description` kebab-case
- The UI talks to the core only through `Command` / `Event` on `gchat_core::Core`. HTTP, protobuf, cookies, and the WebChannel stream stay in `gchat-core`.
- Live runs that should not write to the Google account use `WORKING_READONLY=1` (`just run-readonly`, `just probe`).
- Architecture forks with more than one valid option: ask before implementing

## Git hygiene

- Never commit cookies, MCP tokens, or message text
- Do not add `Co-Authored-By` trailers (including AI tooling attribution)
- Do not amend published history
