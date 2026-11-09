#!/usr/bin/env bash
# CI: works out the build's version and exports it as VERSION. For a tag
# (v1.2.3) it's the tag, written into Cargo.toml so the app reports it;
# otherwise Cargo.toml's version plus the commit.
set -euo pipefail

if [[ "${GITHUB_REF:-}" == refs/tags/v* ]]; then
  version="${GITHUB_REF_NAME#v}"
  if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "::error::Tag $GITHUB_REF_NAME isn't a version like v1.2.3"
    exit 1
  fi
  # Only the first `version = ` line, which is the package's.
  sed -i.bak "1,/^version = /s/^version = \".*\"/version = \"$version\"/" Cargo.toml
  rm Cargo.toml.bak
else
  version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)-${GITHUB_SHA:0:7}"
fi
echo "VERSION=$version" >> "$GITHUB_ENV"
echo "Version $version"
grep -m1 '^version = ' Cargo.toml
