#!/usr/bin/env sh
set -eu

: "${GHCR_IMAGE:?GHCR_IMAGE must be set}"
: "${GITHUB_SHA:?GITHUB_SHA must be set}"

for destination in "$GHCR_IMAGE:latest" "$GHCR_IMAGE:$GITHUB_SHA"; do
  docker tag bugboard-mcp:local "$destination"
  docker push "$destination"
done
