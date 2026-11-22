#!/usr/bin/env bash
# Checks commit messages against CONTRIBUTING.md (Conventional Commits).
#
#   scripts/check-commits.sh <from>..<to>     every commit in the range (CI)
#   scripts/check-commits.sh --file <path>    one message (the commit-msg hook)
set -euo pipefail

types='feat|fix|perf|refactor|test|docs|build|ci|chore|revert'
pattern="^($types)(\([a-z0-9-]+\))?!?: [^ ].*[^.]$"
failed=0

# check <summary> <where>
check() {
  local summary=$1 where=$2
  case "$summary" in
    # Git's own: merges, and squash/fixup commits that get folded in locally.
    "Merge "* | "fixup! "* | "squash! "* | "amend! "*) return 0 ;;
  esac
  if ! [[ "$summary" =~ $pattern ]]; then
    echo "$where: not a Conventional Commit: \"$summary\""
    failed=1
  elif ((${#summary} > 72)); then
    echo "$where: summary is ${#summary} characters (72 at most): \"$summary\""
    failed=1
  fi
}

if [[ "${1:-}" == "--file" ]]; then
  # The first line that isn't a comment (git strips those).
  summary=$(grep -v '^#' "$2" | head -n 1)
  check "$summary" "commit message"
  if ((failed)); then
    echo
    echo "Expected <type>(<scope>): <summary>, with type one of: ${types//|/, }."
    echo "See CONTRIBUTING.md. (Your message is in .git/COMMIT_EDITMSG.)"
  fi
else
  range=${1:?usage: check-commits.sh <from>..<to> | --file <path>}
  # Merge commits are made by GitHub or git, not written.
  while read -r hash summary; do
    check "$summary" "${hash:0:7}"
  done < <(git log --no-merges --format='%H %s' "$range")
  if ((failed)); then
    echo
    echo "Expected <type>(<scope>): <summary>, with type one of: ${types//|/, }."
    echo "See CONTRIBUTING.md; reword with git rebase -i before pushing."
  fi
fi
exit "$failed"
