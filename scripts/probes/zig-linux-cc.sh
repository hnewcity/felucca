#!/bin/sh
# Cross-link probe binaries with a task-local Zig installation on macOS.
set -eu
for argument in "$@"; do
    shift
    case "$argument" in --target=*) ;; *) set -- "$@" "$argument";; esac
done
exec "${FELUCCA_ZIG_PYTHON:?}" -m ziglang cc -target "${FELUCCA_ZIG_TARGET:-x86_64-linux-gnu.2.28}" "$@"
