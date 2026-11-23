# Security Policy

Working holds a Google Chat web session on this computer. Treat the machine and the files in its settings folder (`~/.config/working` on macOS, `$XDG_CONFIG_HOME/working` on Linux, `%APPDATA%\working` on Windows) as the account.

## Trust model

- Session cookies live in `cookies.json` in that folder (mode `0600` on macOS and Linux; on Windows it takes the access list of the user's profile). The MCP secret lives in `mcp-token` beside it. The in-app web views keep their own cookie stores too (on Windows in `%LOCALAPPDATA%\dev.felipefdl.working\WebView2`).
- The app speaks Google Chat as the signed-in user. Sends, read marks, reactions, typing, presence, and DND writes change that account unless `WORKING_READONLY=1`.
- MCP listens on loopback (`127.0.0.1`) by default; the address and port are the user's setting in Settings, Agents. Every request needs the token, and a request from a web page on another origin is refused. An agent that has the token can use the tool groups turned on in Settings: read chat, mail, and calendar, and, with each write switch on, send chat messages, send and change mail, and change events.

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
