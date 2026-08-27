#!/bin/sh
set -e

cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
[ -n "$VERSION" ] || { echo "no version in Cargo.toml" >&2; exit 1; }

jq --argjson v "[$(echo "$VERSION" | tr . ,)]" \
   '.packages[].artifacts[0].version = $v' kpm/repo.json > kpm/repo.json.tmp
mv kpm/repo.json.tmp kpm/repo.json

jq '.packages | to_entries[0] as $e | {
      manifest_version: 2,
      id: $e.key,
      name: $e.value.name,
      author: $e.value.author,
      description: $e.value.description,
      version: $e.value.artifacts[0].version,
      dependencies: $e.value.artifacts[0].dependencies,
      supported_platforms: $e.value.artifacts[0].supported_platforms
    }' kpm/repo.json > kpm/manifest.json
