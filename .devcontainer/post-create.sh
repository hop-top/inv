#!/usr/bin/env bash
set -euo pipefail

# Install go-task (https://github.com/go-task/task)
# Uses curl-pipe pattern — standard for devcontainer setup.
# To pin a version: add "-v vX.Y.Z" after "-b ~/.local/bin".
if ! command -v task &>/dev/null; then
  sh -c "$(curl --location \
    https://taskfile.dev/install.sh)" \
    -- -d -b ~/.local/bin
fi

# Run setup if Taskfile exists
if [ -f Taskfile.yml ] || [ -f Taskfile.yaml ]; then
  task setup
fi
