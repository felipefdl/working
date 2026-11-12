#!/usr/bin/env python3
"""Writes the GitHub Release body for a tag from the conventional commits since the previous release.

Usage: scripts/release_notes.py <tag> [<previous tag>] > notes.md

The release workflow passes the tag of the latest published release as the previous tag, so tags that were never
published are covered too. Without it, a stable tag counts from the previous stable tag (covering what its
release candidates shipped), and a prerelease tag (one with a "-", such as v0.0.7-rc1) from the tag right before
it. Categories follow the release-notes order: Breaking changes, Security, Features, Fixes, Performance, Other.
Empty ones are left out.
"""

import re
import subprocess
import sys

CATEGORIES = ["Breaking changes", "Security", "Features", "Fixes", "Performance", "Other"]
TYPES = {"feat": "Features", "fix": "Fixes", "perf": "Performance", "security": "Security"}
SUBJECT = re.compile(r"^(?P<type>[a-z]+)(?:\((?P<scope>[^)]*)\))?(?P<breaking>!)?:\s*(?P<text>.+)$")


def git(*args: str) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout


def previous_tag(tag: str) -> str | None:
    """The release this one follows: the newest earlier v* tag, a stable one for a stable tag."""
    stable = "-" not in tag
    for candidate in git("tag", "--merged", f"{tag}^", "--list", "v*", "--sort=-v:refname").split():
        if candidate == tag:
            continue
        if stable and "-" in candidate:
            continue
        return candidate
    return None


def notes(tag: str, previous: str | None) -> str:
    previous = previous or previous_tag(tag)
    span = f"{previous}..{tag}" if previous else tag
    sections: dict[str, list[str]] = {name: [] for name in CATEGORIES}
    for subject in git("log", "--no-merges", "--reverse", "--format=%s", span).splitlines():
        subject = subject.strip()
        match = SUBJECT.match(subject)
        if match is None:
            sections["Other"].append(subject)
            continue
        if match["type"] == "chore" and match["scope"] == "release":
            continue
        if match["breaking"]:
            category = "Breaking changes"
        else:
            category = TYPES.get(match["type"], "Other")
        sections[category].append(match["text"].rstrip("."))
    body: list[str] = []
    for name in CATEGORIES:
        entries = sections[name]
        if not entries:
            continue
        body.append(f"### {name}")
        if name == "Breaking changes":
            body += ["", "> [!CAUTION]", "> Update before adopting this release.", ""]
        body += [f"- {entry}" for entry in entries]
        body.append("")
    if not body:
        body = ["### Other", "- rebuild with no changes since the previous release", ""]
    return "\n".join(body)


def main() -> None:
    if len(sys.argv) not in (2, 3):
        sys.exit("usage: release_notes.py <tag> [<previous tag>]")
    previous = sys.argv[2] if len(sys.argv) == 3 and sys.argv[2] else None
    sys.stdout.write(notes(sys.argv[1], previous))


if __name__ == "__main__":
    main()
