#!/bin/bash
# Scan staged changes by default, a full commit with --all, or outgoing changes with --range.
set -euo pipefail

# Add project-specific patterns here.
patterns=('[0-9]{3}-[0-9]{2}-[0-9]{4}')
grep_patterns=()
for pattern in "${patterns[@]}"; do
    grep_patterns+=(-e "$pattern")
done

case "${1:-}" in
    "" | --fix)
        [[ $# -le 1 ]] || exit 2
        scope=(--cached --)
        changes=(--cached)
        ;;
    --all)
        [[ $# -le 2 ]] || exit 2
        tip=$(git rev-parse --verify "${2:-HEAD}^{commit}")
        scope=("$tip" --)
        ;;
    --range)
        [[ $# -eq 3 ]] || exit 2
        base=$(git rev-parse --verify "$2^{commit}")
        tip=$(git rev-parse --verify "$3^{commit}")
        scope=("$tip" --)
        changes=("$base" "$tip")
        ;;
    *)
        echo "Usage: $0 [--all [commit] | --range base tip]" >&2
        exit 2
        ;;
esac

if [[ ${1:-} != --all ]]; then
    path_list=$(mktemp)
    trap 'rm -f "$path_list"' EXIT
    git diff --name-only --no-renames --diff-filter=AMT -z "${changes[@]}" -- > "$path_list"
    while IFS= read -r -d '' path; do
        scope+=("$path")
    done < "$path_list"
    if (( ${#scope[@]} == 2 )); then
        echo "No changed files to scan"
        exit 0
    fi
fi

if git --literal-pathspecs grep -I -l -E "${grep_patterns[@]}" "${scope[@]}"; then
    echo "PII check failed. Review the listed files for sensitive data." >&2
    exit 1
else
    result=$?
    if [[ $result -ne 1 ]]; then
        exit "$result"
    fi
fi
echo "No PII detected"
