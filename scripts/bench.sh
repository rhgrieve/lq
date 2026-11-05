#!/usr/bin/env bash
# Times `lq` against upstream on a theme: fresh processes, N runs each, median wall clock.
# Read-only: nothing is written to the theme, and nothing outside bash and what's being
# measured is needed. The OS file cache stays warm, since dropping it takes root.
#
#   scripts/bench.sh [--runs N] [theme]
#
# Upstream is `shopify theme check --output=json` from your PATH, and Prettier with the Liquid
# plugin from reference/oracle. A missing one is reported and skipped.
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUNS=11
THEME="$ROOT/reference/corpus/horizon"
while [ $# -gt 0 ]; do
  case "$1" in
    --runs) RUNS="$2"; shift 2 ;;
    *) THEME="$1"; shift ;;
  esac
done
THEME="$(cd "$THEME" && pwd)" || exit 2
LQ="${LQ_BIN:-$ROOT/target/release/lq}"
ORACLE="$ROOT/reference/oracle/node_modules"
PRETTIER="$ORACLE/prettier/bin/prettier.cjs"
PLUGIN="$ORACLE/@shopify/prettier-plugin-liquid/dist/index.js"
[ -f "$PLUGIN" ] || PLUGIN="@shopify/prettier-plugin-liquid"
CACHE="${TMPDIR:-/tmp}/lq-bench-prettier-cache"

# Seconds for one run, to the millisecond, of a command run in the theme.
once() {
  local TIMEFORMAT='%3R'
  { time (cd "$THEME" && "$@" >/dev/null 2>&1); } 2>&1
}

# Prints: median min max.
measure() {
  local i samples=""
  for ((i = 0; i < RUNS; i++)); do
    samples="$samples$(once "$@")"$'\n'
  done
  printf '%s' "$samples" | sort -n | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)], a[1], a[NR] }'
}

row() {
  local name="$1"
  shift
  local result median min max
  result="$(measure "$@")"
  read -r median min max <<<"$result"
  printf '| %s | %ss | %ss | %ss |\n' "$name" "$median" "$min" "$max"
  LAST_MEDIAN="$median"
}

ratio() {
  awk -v a="$1" -v b="$2" 'BEGIN { if (b > 0) printf "%.0fx", a / b; else print "n/a" }'
}

[ -x "$LQ" ] || { echo "no binary at $LQ: cargo build --release" >&2; exit 2; }
files="$(cd "$THEME" && find . -name '*.liquid' -not -path './node_modules/*' -not -path './.git/*' | wc -l | tr -d ' ')"
echo "theme: $THEME ($files .liquid files), $RUNS runs each"
echo "machine: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m), $(sysctl -n hw.ncpu 2>/dev/null || nproc) cores, $(uname -sr)"
echo "date: $(date +%Y-%m-%d)"
echo
echo "| command | median | min | max |"
echo "|---|---|---|---|"
row '`lq check --output json`' "$LQ" check --output json
lq_check="$LAST_MEDIAN"
upstream_check=""
if command -v shopify >/dev/null; then
  row '`shopify theme check --output=json`' shopify theme check --output=json
  upstream_check="$LAST_MEDIAN"
else
  echo "| shopify theme check | not on PATH | | |"
fi
row '`lq fmt "**/*.liquid" --check`' "$LQ" fmt '**/*.liquid' --check
lq_fmt="$LAST_MEDIAN"
upstream_fmt=""
if [ -f "$PRETTIER" ]; then
  row '`prettier --check "**/*.liquid"`' node "$PRETTIER" --check --cache-location "$CACHE" --plugin "$PLUGIN" '**/*.liquid'
  upstream_fmt="$LAST_MEDIAN"
else
  echo "| prettier | no reference/oracle | | |"
fi
echo
[ -n "$upstream_check" ] && echo "check: $(ratio "$upstream_check" "$lq_check") faster"
[ -n "$upstream_fmt" ] && echo "fmt: $(ratio "$upstream_fmt" "$lq_fmt") faster"
exit 0
