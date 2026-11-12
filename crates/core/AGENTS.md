# crates/core

`gchat-core`: session, protobuf API, WebChannel stream, disk cache, and loopback MCP. No GPUI types.

## Conventions

- `src/model.rs` is the public resolved shape (names, byte ranges, absolute URLs). `src/convert.rs` maps wire protobuf onto it. `src/proto.rs` is generated from `proto/googlechat.proto`; edit the `.proto`, never the generated file.
- `proto/googlechat.proto` is the hand-maintained reverse-engineered schema (from purple-googlechat). Add fields when the wire needs them; do not invent names or numbers.
- A request the `.proto` cannot express (such as the starred-message world section) goes through `Http::api_json` in the web app's positional JSON (index `n` is field `n + 1`), copied from a captured web request. Batchexecute RPCs go through `Http::page_rpc`.
- The `API_KEY` in `http.rs` is the public Chat web app key. Keep it. Attachment downloads omit `Origin` and `Referer`: with them, Google answers 204.
- Timestamps are `Micros` (microseconds since Unix epoch), as Chat reports them.
- Cache writes go through the atomic temp-file rename in `cache.rs`.
- MCP listens on loopback only. The secret is `~/.config/working/mcp-token`. Tools stay few and named for a person request; a `conversation` argument accepts a name or an id. `WORKING_READONLY=1` still refuses sends even when `allow_send` is true.
