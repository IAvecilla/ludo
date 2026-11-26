#!/usr/bin/env bash
# Times ludo with hyperfine, in three parts:
#
#   1. Problems that can be written in ludo and in other languages, on ludo,
#      on clox, plox and ploxb, on Lua and LuaJIT, and on Gleam.
#   2. Pong played without a window, on ludo and on LÖVE.
#   3. Lists and structs, on ludo, Lua, LuaJIT and Gleam.
#
# Each comparison is a Markdown table in target/bench, and results.md has them
# all.
set -euo pipefail
cd "$(dirname "$0")/.."

for tool in hyperfine lua luajit love gleam erl; do
    if ! command -v "$tool" >/dev/null; then
        echo "$tool is not installed" >&2
        exit 1
    fi
done

cargo build --release --quiet -p ludo
bench/setup.sh
ludo=target/release/ludo
lox=target/bench/lox
out=target/bench
mkdir -p "$out"

# What Gleam compiled, run on the BEAM: `gleam_run MODULE` calls its main.
ebin=bench/gleam/build/dev/erlang
gleam_run() {
    echo "erl -noshell -pa $ebin/ludo_bench/ebin -pa $ebin/gleam_stdlib/ebin -eval 'bench_$1:main(), halt().'"
}

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
        -n ploxb "$lox/ploxb/.venv/bin/ploxb bench/common/$problem.lox" \
        -n lua "lua bench/common/$problem.lua" \
        -n luajit "luajit bench/common/$problem.lua" \
        -n gleam "$(gleam_run "$problem")"
done

# 2. A minute of game: 3600 frames of update, the serve and the shapes to draw.
compare pong \
    -n ludo "$ludo play --frames=3600 examples/pong.ludo" \
    -n love "love bench/love/pong 3600"

# 3.
alone=(lists structs)
for problem in "${alone[@]}"; do
    compare "$problem" \
        -n ludo "$ludo run bench/$problem.ludo" \
        -n lua "lua bench/$problem.lua" \
        -n luajit "luajit bench/$problem.lua" \
        -n gleam "$(gleam_run "$problem")"
done

for name in "${common[@]}" pong "${alone[@]}"; do
    printf '### %s\n\n' "$name"
    cat "$out/$name.md"
    echo
done > "$out/results.md"
echo "Results in $out/results.md"
