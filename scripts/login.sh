#!/bin/sh
# Signs in to Google Chat in a real Chrome window and saves the session cookies the app reads.
# Needs agent-browser (npm i -g agent-browser && agent-browser install) and python3.
set -eu

dir="$HOME/.config/working"
session="working-login"
mkdir -p "$dir"
umask 077

agent-browser --session "$session" --profile "$dir/browser-profile" --headed \
  open "https://accounts.google.com/ServiceLogin?continue=https://chat.google.com/" >/dev/null
echo "Sign in on the Chrome window that just opened. Waiting for Google Chat to load..."
agent-browser --session "$session" wait --url "https://chat.google.com/**" --timeout 600000 >/dev/null

# The browser lists cookies for the page it is on, so they are collected on both hosts: the chat ones for the
# chat API, and the accounts.google.com ones, which let a Meet call sign itself in from this session.
tmp="$(mktemp -d)"
agent-browser --session "$session" cookies --json >"$tmp/chat.json"
agent-browser --session "$session" open "https://accounts.google.com/" >/dev/null
agent-browser --session "$session" cookies --json >"$tmp/accounts.json"
agent-browser --session "$session" close >/dev/null 2>&1 || true

python3 - "$tmp/chat.json" "$tmp/accounts.json" >"$dir/cookies.json" <<'EOF'
import json, sys
keep = {}
for path in sys.argv[1:]:
  for c in json.load(open(path))["data"]["cookies"]:
    if c["domain"] in (".google.com", "chat.google.com", "accounts.google.com"):
      path_attr = c.get("path", "/")
      keep[(c["domain"], c["name"], path_attr)] = {
        "name": c["name"], "value": c["value"], "domain": c["domain"], "path": path_attr,
      }
json.dump(list(keep.values()), sys.stdout, indent=1)
EOF
rm -rf "$tmp"
echo "Saved $dir/cookies.json"
