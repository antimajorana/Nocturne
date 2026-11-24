#!/usr/bin/env bash
# Stage the project license, attribution and bundled third-party notices.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="${1:?package documentation directory required}"
mkdir -p "$dest/licenses"
for file in LICENSE NOTICE ATTRIBUTION.md; do
    install -m 644 "$root/$file" "$dest/$file"
done
for file in "$root"/licenses/*.txt; do
    install -m 644 "$file" "$dest/licenses/${file##*/}"
done
while read -r source name; do
    install -m 644 "$root/$source" "$dest/licenses/$name"
done < "$root/packaging/third-party-notices.txt"
