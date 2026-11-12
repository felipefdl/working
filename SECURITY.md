# Security Policy

Working holds a Google Chat web session on this Mac. Treat the machine and the files under `~/.config/working` as the account.

## Trust model

- Session cookies live in `~/.config/working/cookies.json` (mode `0600`). The MCP secret lives in `~/.config/working/mcp-token`.
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
