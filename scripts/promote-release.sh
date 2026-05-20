#!/usr/bin/env bash
set -euo pipefail

CONFIG="release-please-config.json"

die() { echo "error: $*" >&2; exit 1; }

command -v jq >/dev/null 2>&1 || die "jq is required but not installed"
[ -f "$CONFIG" ] || die "$CONFIG not found"

if ! git diff --cached --quiet; then
  die "staged changes detected. Commit or stash before promoting."
fi

current_stage() {
  jq -r '.packages["."]["prerelease-type"] // "release"' "$CONFIG"
}

valid_next() {
  case "$1" in
    release) echo "alpha" ;;
    alpha)   echo "beta" ;;
    beta)    echo "rc" ;;
    rc)      echo "release" ;;
    *)       die "unknown stage: $1" ;;
  esac
}

apply_stage() {
  local stage="$1"
  local tmp
  tmp=$(mktemp)

  if [ "$stage" = "release" ]; then
    jq '.packages |= with_entries(
      .value |= del(.["prerelease-type"])
    )' "$CONFIG" > "$tmp"
  else
    jq --arg s "$stage" '.packages |= with_entries(
      .value["prerelease-type"] = $s
    )' "$CONFIG" > "$tmp"
  fi

  mv "$tmp" "$CONFIG"
}

CURRENT=$(current_stage)

if [ $# -eq 0 ]; then
  # interactive mode
  NEXT=$(valid_next "$CURRENT")
  echo "Current stage: $CURRENT"
  echo "Next stage:    $NEXT"
  printf "Promote to %s? [y/N] " "$NEXT"
  read -r ans
  case "$ans" in
    [yY]*) ;;
    *) echo "Aborted."; exit 0 ;;
  esac
  apply_stage "$NEXT"
  echo "Promoted to $NEXT"
  git add "$CONFIG"
  git commit -m "chore(release): promote to $NEXT" -- "$CONFIG"
else
  TARGET="$1"
  NEXT=$(valid_next "$CURRENT")
  [ "$TARGET" = "$NEXT" ] || \
    die "invalid transition: $CURRENT -> $TARGET (expected $NEXT)"
  apply_stage "$TARGET"
  echo "Promoted to $TARGET"
  git add "$CONFIG"
  git commit -m "chore(release): promote to $TARGET" -- "$CONFIG"
fi
