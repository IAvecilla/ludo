#!/usr/bin/env bash
# Times ludo with hyperfine, in four parts:
#
#   1. Problems that can be written in ludo and in Lox, on ludo and on clox,
#      plox and ploxb, which setup.sh builds.
#   2. The Pratt parser against the ladder.
#   3. Pong played without a window.
#   4. Problems of ludo alone, with lists and structs.
#
# Each comparison is a Markdown table in target/bench, and results.md has them
# all.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v hyperfine >/dev/null; then
    echo "hyperfine is not installed: cargo install hyperfine --locked" >&2
    exit 1
fi

cargo build --release --quiet -p ludo
bench/setup.sh
ludo=target/release/ludo
lox=target/bench/lox
out=target/bench
mkdir -p "$out"
bench/big.sh "$out/big.ludo"

# compare NAME [hyperfine arguments...]
compare() {
    local name=$1
    shift
    echo "== $name"
    hyperfine --shell=none --warmup 1 --export-markdown "$out/$name.md" "$@"
}

# 1. Each one runs at least three times, and as many more as fit in three
# seconds: the ones in Python take seconds each.
common=(fib loops mandelbrot strings)
for problem in "${common[@]}"; do
    compare "$problem" --min-runs 3 \
        -n ludo "$ludo run bench/common/$problem.ludo" \
        -n clox "$lox/craftinginterpreters/build/clox bench/common/$problem.lox" \
        -n plox "$lox/plox/.venv/bin/plox bench/common/$problem.lox" \
        -n ploxb "$lox/ploxb/.venv/bin/ploxb bench/common/$problem.lox"
done

# 2. Running a 30000 line program: only the parser changes in each pair.
compare parser \
    -n ladder "$ludo run --parser=ladder $out/big.ludo" \
    -n pratt "$ludo run --parser=pratt $out/big.ludo"

# 3 and 4. A minute of game, 3600 frames of update, sequences and draw, and
# the problems of ludo alone, in one table.
compare alone \
    -n pong "$ludo play --frames=3600 examples/pong.ludo" \
    -n lists "$ludo run bench/lists.ludo" \
    -n structs "$ludo run bench/structs.ludo"

for name in "${common[@]}" parser alone; do
    printf '### %s\n\n' "$name"
    cat "$out/$name.md"
    echo
done > "$out/results.md"
echo "Results in $out/results.md"
