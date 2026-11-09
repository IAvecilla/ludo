#!/usr/bin/env bash
# Writes a long program to $1 for the parser benchmark: lines of expressions
# that go through every level of precedence, each one using the line before.
lines=${2:-20000}
{
    echo "let x0 = 7"
    for ((i = 1; i <= lines; i++)); do
        p=$((i - 1))
        if ((i % 2)); then
            echo "let x$i = (x$p * 3 + 7) % 1000 - x$p / 2 + -(x$p % 5) * 2"
        else
            echo "let b$i = x$p > 10 and x$p < 900 or x$p == 0 and !(x$p != 3)"
            echo "let x$i = if b$i { x$p + 1 } else { x$p - 1 }"
        fi
    done
    echo "print(x$lines)"
} > "$1"
