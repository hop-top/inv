#!/usr/bin/env bash
set -euo pipefail

# -------------------------------------------------------
# init.sh — Post-clone project initializer
# Replaces {{placeholder}} tokens, configures license,
# optionally prunes unselected languages (polyglot).
# Compatible with macOS + Linux.
#
# Environment overrides (for CI/automation):
#   INIT_APP_NAME, INIT_DESCRIPTION, INIT_AUTHOR_NAME,
#   INIT_AUTHOR_EMAIL, INIT_LICENSE, INIT_MODULE_PREFIX,
#   INIT_LANGUAGES (comma-separated, polyglot only)
# -------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR"
YEAR="$(date +%Y)"

# shellcheck source=../lib.sh
source "$SCRIPT_DIR/../lib.sh"

# --- Detect polyglot -----------------------------------

is_polyglot=false
if [ -d "$PROJECT_DIR/go" ] && \
   [ -d "$PROJECT_DIR/ts" ] && \
   [ -d "$PROJECT_DIR/py" ] && \
   [ -d "$PROJECT_DIR/rs" ]; then
  is_polyglot=true
fi

# --- Prompts (env vars override for automation) --------

default_name="$(basename "$PROJECT_DIR")"

app_name="${INIT_APP_NAME:-}"
[ -z "$app_name" ] && prompt app_name "App name" "$default_name"

description="${INIT_DESCRIPTION:-}"
[ -z "$description" ] && prompt description "Description" ""

author_name="${INIT_AUTHOR_NAME:-}"
[ -z "$author_name" ] && prompt author_name "Author name" ""

author_email="${INIT_AUTHOR_EMAIL:-}"
[ -z "$author_email" ] && prompt author_email "Author email" ""

license="${INIT_LICENSE:-}"
[ -z "$license" ] && prompt_choice license "License" "MIT/Apache-2.0" "MIT"

module_prefix="${INIT_MODULE_PREFIX:-}"
[ -z "$module_prefix" ] && prompt module_prefix \
  "Module prefix (e.g. github.com/user)" ""

selected_langs=""
if [ "$is_polyglot" = true ]; then
  selected_langs="${INIT_LANGUAGES:-}"
  [ -z "$selected_langs" ] && prompt_multi selected_langs \
    "Languages (comma-separated)" \
    "go,ts,py,rs" "go,ts,py,rs"
fi

# --- Derived tokens ------------------------------------

# Uppercase app name for env var prefixes (e.g. my-app -> MY_APP)
app_name_upper="$(echo "$app_name" | tr '[:lower:]-' '[:upper:]_')"

# --- Replace tokens ------------------------------------

echo "Replacing tokens..."

replace_token "{{app_name_upper}}" "$app_name_upper"
replace_token "{{app_name}}" "$app_name"
replace_token "{{description}}" "$description"
replace_token "{{author_name}}" "$author_name"
replace_token "{{author_email}}" "$author_email"
replace_token "{{module_prefix}}" "$module_prefix"
replace_token "{{license}}" "$license"
replace_token "{{year}}" "$YEAR"

# --- License -------------------------------------------

echo "Configuring license..."

if [ "$license" = "Apache-2.0" ]; then
  cp "$PROJECT_DIR/LICENSE-Apache-2.0" \
    "$PROJECT_DIR/LICENSE"
else
  cp "$PROJECT_DIR/LICENSE-MIT" \
    "$PROJECT_DIR/LICENSE"
fi

# Clean up license templates
rm -f "$PROJECT_DIR/LICENSE-MIT" \
  "$PROJECT_DIR/LICENSE-Apache-2.0"

# --- Module paths --------------------------------------

# Fix module paths at root (single-lang templates)
if [ -f "$PROJECT_DIR/go.mod" ]; then
  sedi \
    "s|module .*|module ${module_prefix}/${app_name}|" \
    "$PROJECT_DIR/go.mod"
fi

if [ -f "$PROJECT_DIR/package.json" ]; then
  sedi \
    "s|\"name\": \".*\"|\"name\": \"${app_name}\"|" \
    "$PROJECT_DIR/package.json"
fi

if [ -f "$PROJECT_DIR/pyproject.toml" ]; then
  sedi \
    "s|name = \".*\"|name = \"${app_name}\"|" \
    "$PROJECT_DIR/pyproject.toml"
fi

# Fix module paths in polyglot subdirs
if [ "$is_polyglot" = true ]; then
  if [ -f "$PROJECT_DIR/go/go.mod" ]; then
    sedi \
      "s|module .*|module ${module_prefix}/${app_name}|" \
      "$PROJECT_DIR/go/go.mod"
  fi

  if [ -f "$PROJECT_DIR/ts/package.json" ]; then
    sedi \
      "s|\"name\": \".*\"|\"name\": \"${app_name}\"|" \
      "$PROJECT_DIR/ts/package.json"
  fi

  if [ -f "$PROJECT_DIR/py/pyproject.toml" ]; then
    sedi \
      "s|name = \".*\"|name = \"${app_name}\"|" \
      "$PROJECT_DIR/py/pyproject.toml"
  fi
fi

# --- Python src dir rename -----------------------------

# Single-lang template
if [ -d "$PROJECT_DIR/src/{{app_name}}" ]; then
  mv "$PROJECT_DIR/src/{{app_name}}" \
    "$PROJECT_DIR/src/${app_name}"
fi

# Polyglot template
if [ -d "$PROJECT_DIR/py/src/{{app_name}}" ]; then
  mv "$PROJECT_DIR/py/src/{{app_name}}" \
    "$PROJECT_DIR/py/src/${app_name}"
fi

# --- Polyglot: prune unselected languages --------------

if [ "$is_polyglot" = true ]; then
  echo "Pruning unselected languages..."

  # Use comma-delimited string for Bash 3.2 compat (no assoc arrays)
  keep_langs=",$(echo "$selected_langs" | tr -d ' '),"

  for lang in go ts py rs; do
    if [[ "$keep_langs" != *",$lang,"* ]]; then
      echo "  Removing $lang..."

      # Remove language directory
      rm -rf "${PROJECT_DIR:?}/${lang}"

      # Remove CI workflows
      rm -f "$PROJECT_DIR/.github/workflows/ci-${lang}.yml"
      rm -f "$PROJECT_DIR/.github/workflows/release-${lang}.yml"

      # Remove delegation lines from root Makefile
      if [ -f "$PROJECT_DIR/Makefile" ]; then
        sedi "/\$(MAKE) -C ${lang}/d" "$PROJECT_DIR/Makefile"
      fi
    fi
  done
fi

# --- Git init ------------------------------------------

if [ ! -d "$PROJECT_DIR/.git" ]; then
  echo "Initializing git repository..."
  git -C "$PROJECT_DIR" init
fi

# Use prompted author info for local git config
git -C "$PROJECT_DIR" config user.name "$author_name"
git -C "$PROJECT_DIR" config user.email "$author_email"

echo "Creating initial commit..."
git -C "$PROJECT_DIR" add -A
git -C "$PROJECT_DIR" commit -m \
  "feat: initialize ${app_name}"

# --- Cleanup -------------------------------------------

echo "Cleaning up..."
rm -f "$0"

echo ""
echo "Done! ${app_name} is ready."
echo "Run 'cd ${PROJECT_DIR}' to get started."
