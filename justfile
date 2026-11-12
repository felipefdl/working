# Working development recipes.
# Primary gate: `just check`

set shell := ["bash", "-euo", "pipefail", "-c"]

default:
  @just --list

# Format check (does not write).
fmt-check:
  cargo fmt --all --check

# Format write.
fmt:
  cargo fmt --all

# Clippy with deny warnings for all targets and features.
clippy:
  cargo clippy --workspace --all-targets --all-features -- -D warnings

# Deterministic offline tests.
# --no-tests=pass: a crate with zero tests still exercises nextest.
test:
  TZ=UTC cargo nextest run --workspace --no-tests=pass

# Lint and test one package (mid-change loop). Not the gate.
task pkg:
  cargo clippy -p {{pkg}} --all-targets -- -D warnings
  TZ=UTC cargo nextest run -p {{pkg}} --no-tests=pass

# Filtered test run: `just t 'test(save)'`.
t expr:
  TZ=UTC cargo nextest run --workspace -E '{{expr}}'

# Supply-chain and unused-deps checks.
deny:
  cargo deny check

audit:
  cargo audit

machete:
  cargo machete

# Verify every workspace member sets [lints] workspace = true.
lints-inherit:
  #!/usr/bin/env bash
  set -euo pipefail
  fail=0
  while IFS= read -r -d '' f; do
    if ! grep -qF '[lints]' "$f" || ! grep -qF 'workspace = true' "$f"; then
      echo "missing [lints] workspace = true: $f" >&2
      fail=1
    fi
  done < <(find crates -name Cargo.toml -print0)
  if [[ "$fail" -ne 0 ]]; then
    exit 1
  fi
  echo "all member crates inherit workspace lints"

# Optional Chrome sign-in; the app also writes this file from the welcome screen.
login:
  ./scripts/login.sh

# Build optimized, wrap as Working.app, and open it.
run:
  ./scripts/bundle.sh
  open target/release/Working.app

# Run without writing to the account: no sends, read marks, reactions, or typing notices.
run-readonly:
  ./scripts/bundle.sh
  WORKING_READONLY=1 target/release/Working.app/Contents/MacOS/Working

# Run with a made-up workspace instead of a Google account: no sign-in, no network, no real cache or settings.
demo:
  ./scripts/bundle.sh
  WORKING_DEMO=1 target/release/Working.app/Contents/MacOS/Working

# Rebuild the top-bar logo (needs resvg) and the README icon (needs Xcode).
logo:
  ./scripts/logo.sh

# Read-only protocol smoke run: prints counts and event types, never message text.
probe seconds="20":
  WORKING_READONLY=1 WORKING_TRACE=1 cargo run -p gchat-core --example probe -- {{seconds}}

# Bump the version, commit, tag, and push. The tag runs .github/workflows/publish.yml: build, sign, notarize, write
# the notes from the commits, publish the release, and update the Homebrew cask. A version with a "-" (0.0.7-rc1)
# becomes a draft prerelease instead, with no cask update.
release version:
  #!/usr/bin/env bash
  set -euo pipefail
  if [[ ! "{{version}}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
    echo "version must be X.Y.Z or X.Y.Z-pre" >&2
    exit 1
  fi
  if [[ "$(git branch --show-current)" != main ]]; then
    echo "release from main" >&2
    exit 1
  fi
  if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "commit or stash changes first" >&2
    exit 1
  fi
  sed -i '' 's/^version = "[^"]*"/version = "{{version}}"/' Cargo.toml
  cargo update -w
  git add Cargo.toml Cargo.lock
  git commit -m "chore(release): v{{version}}"
  git tag "v{{version}}"
  git push origin HEAD "v{{version}}"

# Ordered cheapest first: a formatting failure reports in seconds instead of
# after the compile steps.
#
# Full quality gate (docs/rust-quality.md).
check: fmt-check lints-inherit deny audit machete clippy test
  @echo "quality gate passed"
