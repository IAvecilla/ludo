#!/usr/bin/env bash
# Times ludo with hyperfine: the two parsers, the VM against the interpreter on
# three programs, and pong played without a window on each backend. Each
# comparison is a Markdown table in target/bench, and results.md has them all.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v hyperfine >/dev/null; then
    echo "hyperfine is not installed: cargo install hyperfine --locked" >&2
    exit 1
fi

cargo build --release --quiet -p ludo
ludo=target/release/ludo
out=target/bench
mkdir -p "$out"
bench/big.sh "$out/big.ludo"

# compare NAME [hyperfine arguments...]
compare() {
    local name=$1
    shift
    echo "== $name"
    hyperfine --shell=none --warmup 2 --export-markdown "$out/$name.md" "$@"
}

# Parsing and checking a 30000 line program: only the parser changes.
compare parser \
    -n ladder "$ludo check --parser=ladder $out/big.ludo" \
    -n pratt "$ludo check --parser=pratt $out/big.ludo"

for program in fib lists structs; do
    compare "$program" \
        -n interpreter "$ludo run --interpreter bench/$program.ludo" \
        -n vm "$ludo run bench/$program.ludo"
done

# A minute of game: 3600 frames of update, sequences and draw.
compare pong \
    -n interpreter "$ludo play --interpreter --frames=3600 examples/pong.ludo" \
    -n vm "$ludo play --frames=3600 examples/pong.ludo"

for name in parser fib lists structs pong; do
    printf '### %s\n\n' "$name"
    cat "$out/$name.md"
    echo
done > "$out/results.md"
echo "Results in $out/results.md"
