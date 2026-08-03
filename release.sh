#!/usr/bin/env bash

set -euo pipefail

repository_root="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "error: release.sh must be run inside a Git repository" >&2
  exit 1
}
cd "${repository_root}"

if ! git diff --quiet ||
  ! git diff --cached --quiet ||
  [[ -n "$(git ls-files --others --exclude-standard)" ]]; then
  echo "error: all changes must be committed before creating a release tag" >&2
  git status --short >&2
  exit 1
fi

release_tag="$(date +'%Y%m%dT%H%M%S%z')"
if git show-ref --verify --quiet "refs/tags/${release_tag}"; then
  echo "error: tag ${release_tag} already exists" >&2
  exit 1
fi

git tag --annotate "${release_tag}" --message "Release ${release_tag}" HEAD

echo "Created release tag ${release_tag} at $(git rev-parse --short HEAD)."
read -r -p "Push ${release_tag} to origin now? [y/N] " push_answer || push_answer=""
case "${push_answer}" in
  y | Y | yes | Yes | YES)
    git push origin "${release_tag}" && git push
    echo "Pushed ${release_tag}; the GitHub Release workflow will start."
    ;;
  *)
    echo "Tag not pushed. Push it later with:"
    echo "  git push origin ${release_tag}"
    ;;
esac
