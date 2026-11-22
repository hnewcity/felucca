#!/bin/sh
# The function is invoked only after the complete script has been downloaded.
felucca_connect() {
    set -eu
    umask 077
    [ "$#" = 5 ] || { printf '%s\n' {{invalid}} >&2; exit 1; }
    felucca_origin=$1 felucca_device=$2 felucca_token=$3 felucca_config=$4 felucca_language=$5
    case "$felucca_device" in ''|.|..|*[!a-zA-Z0-9_.-]*) printf '%s\n' {{invalid}} >&2; exit 1;; esac
    case "$felucca_language" in en|zh|ja|ko|fr|es|ru) ;; *) exit 1;; esac
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64) felucca_target=aarch64-apple-darwin;;
        Darwin/x86_64) felucca_target=x86_64-apple-darwin;;
        Linux/aarch64|Linux/arm64) felucca_target=aarch64-unknown-linux-gnu;;
        Linux/x86_64) felucca_target=x86_64-unknown-linux-gnu;;
        *) printf '%s\n' {{unsupported}} >&2; exit 1;;
    esac
    felucca_digest() {
        if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1;
        else shasum -a 256 "$1" | cut -d ' ' -f 1; fi
    }
    felucca_fetch() {
        curl --fail --silent --show-error --connect-timeout 15 --max-time 600 --retry 3 \
            --header "Authorization: Bearer $felucca_token" --header "Accept-Language: $felucca_language" "$@"
    }
    felucca_url="$felucca_origin/api/connect/$felucca_device/$felucca_target"
    felucca_sha=$(felucca_fetch "$felucca_url/sha256") || { printf '%s\n' {{unavailable}} >&2; exit 1; }
    case "$felucca_sha" in ''|*[!a-f0-9]*) printf '%s\n' {{checksum}} >&2; exit 1;; esac
    [ "${#felucca_sha}" = 64 ] || exit 1
    felucca_base="${FELUCCA_DATA_DIR:-${XDG_DATA_HOME:-${HOME:?}/.local/share}/felucca}"
    mkdir -p "$felucca_base"
    felucca_work=$(mktemp -d "$felucca_base/.connect-XXXXXXXX")
    trap 'rm -rf -- "$felucca_work"' EXIT
    trap 'exit 130' INT TERM HUP
    printf '%s' "$felucca_origin" > "$felucca_work/origin"
    felucca_server=$(felucca_digest "$felucca_work/origin")
    felucca_folder="$felucca_base/$felucca_server/$felucca_device"
    felucca_binary="$felucca_base/bin/$felucca_sha/felucca-agent"
    mkdir -p "$felucca_folder" "$(dirname "$felucca_binary")"
    if [ ! -f "$felucca_binary" ] || [ "$(felucca_digest "$felucca_binary")" != "$felucca_sha" ]; then
        felucca_fetch --output "$felucca_work/felucca-agent" "$felucca_url/binary" || { printf '%s\n' {{unavailable}} >&2; exit 1; }
        [ "$(felucca_digest "$felucca_work/felucca-agent")" = "$felucca_sha" ] || { printf '%s\n' {{checksum}} >&2; exit 1; }
        chmod 700 "$felucca_work/felucca-agent"
        mv -f "$felucca_work/felucca-agent" "$felucca_binary"
    fi
    # Homebrew/GNU utilities can precede Apple's utilities on macOS too.
    if base64 --decode </dev/null >/dev/null 2>&1; then
        printf '%s' "$felucca_config" | base64 --decode > "$felucca_work/connection.json"
    else
        printf '%s' "$felucca_config" | base64 -D > "$felucca_work/connection.json"
    fi
    "$felucca_binary" --config "$felucca_work/connection.json" --check --lang "$felucca_language"
    mv -f "$felucca_work/connection.json" "$felucca_folder/connection.json"
    printf '%s\n' {{starting}}
    rm -rf -- "$felucca_work"
    trap - EXIT INT TERM HUP
    exec "$felucca_binary" --config "$felucca_folder/connection.json" --exclusive --lang "$felucca_language"
}
felucca_connect "$@"
