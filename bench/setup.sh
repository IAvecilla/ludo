#!/usr/bin/env bash
# Fetches and builds the Lox implementations that the common problems compare
# ludo against, each at a fixed commit, into target/bench/lox:
#
#   clox   the bytecode VM of Crafting Interpreters, in C, built with its make
#   plox   the course's tree-walking interpreter, in Python
#   ploxb  the course's bytecode VM, in Python: the branch with functions
#
# plox and ploxb are installed with uv from their lockfiles, as their READMEs
# say. Running it again only redoes what changed.
set -euo pipefail
cd "$(dirname "$0")/.."

dir=target/bench/lox
mkdir -p "$dir"

for tool in git make cc uv; do
    if ! command -v "$tool" >/dev/null; then
        echo "$tool is needed to set up the Lox implementations" >&2
        exit 1
    fi
done

# fetch NAME URL COMMIT [PATH...]: checks out one commit of a repository,
# without its history and, if PATHs are given, only those folders.
fetch() {
    local name=$1 url=$2 commit=$3
    shift 3
    local repo=$dir/$name
    if [ "$(git -C "$repo" rev-parse HEAD 2>/dev/null)" = "$commit" ]; then
        return
    fi
    echo "== fetching $name"
    rm -rf "$repo"
    git init -q "$repo"
    if [ $# -gt 0 ]; then
        git -C "$repo" sparse-checkout set "$@"
    fi
    git -C "$repo" fetch -q --depth 1 --filter=blob:none "$url" "$commit"
    git -C "$repo" checkout -q FETCH_HEAD
}

fetch craftinginterpreters https://github.com/munificent/craftinginterpreters.git \
    4a840f70f69c6ddd17cfef4f6964f8e1bcd8c3d4 c util
fetch plox https://github.com/FdelMazo/plox.git \
    734f4d2adaf708c503dc5d17e88315e273c2f0ab
fetch ploxb https://github.com/FdelMazo/ploxb.git \
    49d1b13126802197edb07881b28a1eed722e7b95

# What `make clox` runs, without the rest of the Makefile, which needs the
# book's Dart tools: the release build, with -O3, NaN boxing and no tracing.
if [ ! -x "$dir/craftinginterpreters/build/clox" ]; then
    echo "== building clox"
    make -s -C "$dir/craftinginterpreters" -f util/c.make \
        NAME=clox MODE=release SOURCE_DIR=c >/dev/null
fi

for name in plox ploxb; do
    if [ ! -x "$dir/$name/.venv/bin/$name" ]; then
        echo "== installing $name"
        (cd "$dir/$name" && uv sync --frozen --quiet)
    fi
done
