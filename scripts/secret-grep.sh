#!/usr/bin/env bash
# secret-grep.sh — must exit 0 with ZERO hits before anything ships
# (DESIGN.md §5, §10, §13).
#
# Flags anything key- or token-shaped in the repo (source, scripts, tests,
# fixtures, and the packed dist/harness.html):
#   1. OpenRouter key shapes: sk-or-<run of key chars> (>=16)
#   2. OPENROUTER_API_KEY= / HARNESS_ACCESS_TOKEN= with a non-empty value
#      (.env.example ships empty values and is allowed; local .env is never
#      scanned and never committed)
#   3. Long bearer literals: Bearer <token-shaped run>
#
# Excluded: build/dev dirs only (.git, target, node_modules) and .env files.

set -u
cd "$(dirname "$0")/.."

if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    FILES=$(git ls-files --cached --others --exclude-standard)
else
    FILES=$(find . -type f \
        -not -path './.git/*' \
        -not -path '*/target/*' \
        -not -path './node_modules/*' \
        -not -name '.env')
fi

status=0
scan() {
    local label="$1" pattern="$2" hits="" found
    while IFS= read -r f; do
        [ -z "$f" ] && continue
        # This script itself contains only the pattern definitions.
        [ "$(basename "$f")" = "secret-grep.sh" ] && continue
        found=$(grep -HnE "$pattern" "$f" 2>/dev/null || true)
        if [ -n "$found" ]; then
            hits="${hits}${found}"$'\n'
        fi
    done <<< "$FILES"
    if [ -n "$hits" ]; then
        echo "SECRET-GREP HIT [$label]:"
        echo "$hits"
        status=1
    fi
}

scan 'openrouter-key-shape' 'sk-or-[A-Za-z0-9_-]{16,}'
scan 'openrouter-env-value' 'OPENROUTER_API_KEY=[^[:space:]"'"'"']{8,}'
scan 'harness-env-value'    'HARNESS_ACCESS_TOKEN=[^[:space:]"'"'"']{8,}'
scan 'bearer-literal'       'Bearer [A-Za-z0-9._-]{24,}'

if [ "$status" -eq 0 ]; then
    echo "secret-grep: clean (0 hits)"
fi
exit "$status"
