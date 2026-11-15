# Security Policy

Working holds a Google Chat web session on this computer. Treat the machine and the files in its settings folder (`~/.config/working` on macOS, `$XDG_CONFIG_HOME/working` on Linux, `%APPDATA%\working` on Windows) as the account.

## Trust model

- Session cookies live in `cookies.json` in that folder (mode `0600` on macOS and Linux; on Windows it takes the access list of the user's profile). The MCP secret lives in `mcp-token` beside it. The in-app web views keep their own cookie stores too (on Windows in `%LOCALAPPDATA%\dev.felipefdl.working\WebView2`).
- The app speaks Google Chat as the signed-in user. Sends, read marks, reactions, typing, presence, and DND writes change that account unless `WORKING_READONLY=1`.
- MCP listens on loopback only. A local agent that has the token can read chat, and can send when sending is enabled in Settings.

## What this is not

- Not a Google product
- Not a multi-user or remote-access service
- Not a cloud API with its own accounts

## Reporting issues

Report security issues privately through [GitHub's security advisory form](https://github.com/felipefdl/working/security/advisories/new). Do not report security vulnerabilities on the public issue tracker. Do not include cookie values, MCP tokens, or message text.

## Vulnerability coordination

Confirmed vulnerabilities are coordinated through the [GitHub Security Advisory](https://docs.github.com/en/code-security/security-advisories) opened for the issue.

## Security advisories

Security fixes are disclosed through [GitHub release notes](https://github.com/felipefdl/working/releases).
