#!/bin/sh
# Optional development wrapper: keep dependency downloads and temp files on the
# same volume as the checkout, useful when the system volume is nearly full.
set -eu
felucca_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export CARGO_HOME="${FELUCCA_CARGO_CACHE:-$felucca_root/target/cargo-home}"
export TMPDIR="$felucca_root/target/tmp"
mkdir -p "$CARGO_HOME" "$TMPDIR"
exec cargo "$@"
