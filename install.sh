#!/bin/sh

set -eu

REPO="agentsmith-project/llm-universal-proxy"
DEFAULT_RELEASE_TAG="__LLMUP_RELEASE_TAG__"
INSTALL_RELEASE_TAG="${LLMUP_INSTALL_RELEASE_TAG:-$DEFAULT_RELEASE_TAG}"
BIN_DIR="${HOME:?HOME must be set}/.local/bin"
MODIFY_PATH=1
TMP_DIR=""

log() {
    printf '%s\n' "$*"
}

fail() {
    printf 'llmup installer error: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR"
    fi
}

trap cleanup 0 HUP INT TERM

usage() {
    cat <<'EOF'
Usage: sh install.sh [--bin-dir DIR] [--no-modify-path]

Installs llm-universal-proxy and creates llmup-config, llmup-codex, and
llmup-claude aliases in the target bin directory.

Options:
  --bin-dir DIR       Install into DIR instead of ~/.local/bin
  --no-modify-path   Do not update ~/.profile
  -h, --help         Show this help
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --bin-dir)
            [ "$#" -ge 2 ] || fail "--bin-dir requires a directory"
            BIN_DIR="$2"
            shift 2
            ;;
        --bin-dir=*)
            BIN_DIR=${1#--bin-dir=}
            [ -n "$BIN_DIR" ] || fail "--bin-dir requires a directory"
            shift
            ;;
        --no-modify-path)
            MODIFY_PATH=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "unknown option: $1"
            ;;
    esac
done

absolute_path() {
    case "$1" in
        /*)
            printf '%s\n' "$1"
            ;;
        *)
            printf '%s/%s\n' "$(pwd)" "$1"
            ;;
    esac
}

BIN_DIR=$(absolute_path "$BIN_DIR")

path_contains() {
    target=$1
    old_ifs=$IFS
    IFS=:
    for entry in ${PATH:-}; do
        if [ "$entry" = "$target" ]; then
            IFS=$old_ifs
            return 0
        fi
    done
    IFS=$old_ifs
    return 1
}

shell_quote() {
    printf "'"
    printf '%s' "$1" | sed "s/'/'\\\\''/g"
    printf "'"
}

detect_platform() {
    uname_s=${LLMUP_INSTALL_UNAME_S:-$(uname -s)}
    uname_m=${LLMUP_INSTALL_UNAME_M:-$(uname -m)}
    detected_os=

    case "$uname_s" in
        Linux*)
            asset_os=linux
            osrelease_path=${LLMUP_INSTALL_PROC_OSRELEASE:-/proc/sys/kernel/osrelease}
            if [ -r "$osrelease_path" ]; then
                osrelease=$(cat "$osrelease_path" 2>/dev/null || printf '')
                case "$osrelease" in
                    *[Mm]icrosoft*|*WSL*)
                        detected_os=wsl
                        ;;
                esac
            fi
            ;;
        Darwin*)
            asset_os=macos
            ;;
        *)
            fail "unsupported operating system: $uname_s"
            ;;
    esac

    case "$uname_m" in
        x86_64|amd64)
            asset_arch=x86_64
            ;;
        aarch64|arm64)
            asset_arch=aarch64
            ;;
        *)
            fail "unsupported architecture: $uname_m"
            ;;
    esac

    asset_name="llm-universal-proxy-${asset_os}-${asset_arch}.tar.gz"
}

download_file() {
    url=$1
    dest=$2

    case "$url" in
        file://*)
            src=${url#file://}
            cp "$src" "$dest" || fail "failed to copy $url"
            ;;
        *)
            if command -v curl >/dev/null 2>&1; then
                curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$dest" || fail "failed to download $url"
            elif command -v wget >/dev/null 2>&1; then
                wget -qO "$dest" "$url" || fail "failed to download $url"
            else
                fail "missing required downloader: curl or wget"
            fi
            ;;
    esac
}

sha256_of_file() {
    path=$1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$path" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$path" | awk '{print $1}'
    else
        fail "missing required SHA-256 tool: sha256sum or shasum"
    fi
}

read_expected_sha256() {
    sha_file=$1
    expected=$(awk 'NF { print $1; exit }' "$sha_file")
    expected=$(printf '%s' "$expected" | tr 'ABCDEF' 'abcdef')

    case "$expected" in
        ""|*[!0123456789abcdef]*)
            fail "invalid .sha256 file"
            ;;
    esac
    if [ "${#expected}" -ne 64 ]; then
        fail "invalid .sha256 file"
    fi

    printf '%s\n' "$expected"
}

validate_archive() {
    archive=$1
    entries_file=$2

    tar -tzf "$archive" > "$entries_file" || fail "failed to list archive"

    count=0
    while IFS= read -r entry; do
        [ -n "$entry" ] || continue
        count=$((count + 1))

        case "$entry" in
            /*)
                fail "absolute path in archive: $entry"
                ;;
            ..|../*|*/..|*/../*)
                fail "path traversal in archive: $entry"
                ;;
        esac

        if [ "$entry" != "llm-universal-proxy" ]; then
            fail "unexpected archive entry: $entry"
        fi
    done < "$entries_file"

    if [ "$count" -ne 1 ]; then
        fail "unexpected archive entry count: $count"
    fi
}

release_base_url() {
    if [ -n "${LLMUP_INSTALL_BASE_URL:-}" ]; then
        printf '%s\n' "$LLMUP_INSTALL_BASE_URL"
        return
    fi

    if [ "$INSTALL_RELEASE_TAG" = "__LLMUP_RELEASE_TAG__" ] || [ -z "$INSTALL_RELEASE_TAG" ]; then
        printf 'https://github.com/%s/releases/latest/download\n' "$REPO"
    else
        printf 'https://github.com/%s/releases/download/%s\n' "$REPO" "$INSTALL_RELEASE_TAG"
    fi
}

ensure_profile_path() {
    profile=$HOME/.profile
    marker_begin="# >>> llmup installer >>>"
    marker_end="# <<< llmup installer <<<"

    if [ -f "$profile" ] && grep -F "$marker_begin" "$profile" >/dev/null 2>&1; then
        return
    fi

    quoted_bin_dir=$(shell_quote "$BIN_DIR")
    {
        printf '\n%s\n' "$marker_begin"
        printf 'llmup_path=%s\n' "$quoted_bin_dir"
        printf 'case ":$PATH:" in\n'
        printf '  *:"$llmup_path":*) ;;\n'
        printf '  *) PATH="$llmup_path:$PATH" ;;\n'
        printf 'esac\n'
        printf 'export PATH\n'
        printf 'unset llmup_path\n'
        printf '%s\n' "$marker_end"
    } >> "$profile" || fail "failed to update $profile"
}

check_target_conflicts() {
    manifest=$BIN_DIR/.llmup-install-manifest
    managed=0
    if [ -f "$manifest" ] && grep -F "managed-by=llmup-install" "$manifest" >/dev/null 2>&1; then
        managed=1
    fi

    for name in llm-universal-proxy llmup-config llmup-codex llmup-claude; do
        target=$BIN_DIR/$name
        if [ -e "$target" ] || [ -L "$target" ]; then
            if [ "$managed" -ne 1 ]; then
                fail "refusing to overwrite existing file: $target"
            fi
        fi
    done
}

install_binary_and_aliases() {
    extract_dir=$1
    primary=$BIN_DIR/llm-universal-proxy
    tmp_primary=$BIN_DIR/.llm-universal-proxy.$$

    mkdir -p "$BIN_DIR" || fail "failed to create bin-dir: $BIN_DIR"
    [ -d "$BIN_DIR" ] || fail "bin-dir is not a directory: $BIN_DIR"
    [ -w "$BIN_DIR" ] || fail "target bin-dir is not writable: $BIN_DIR"

    check_target_conflicts

    cp "$extract_dir/llm-universal-proxy" "$tmp_primary" || fail "failed to stage binary"
    chmod 755 "$tmp_primary" || fail "failed to mark binary executable"
    mv -f "$tmp_primary" "$primary" || fail "failed to install binary atomically"

    for alias in llmup-config llmup-codex llmup-claude; do
        tmp_alias=$BIN_DIR/.$alias.$$
        rm -f "$tmp_alias"
        if ln -s "llm-universal-proxy" "$tmp_alias" 2>/dev/null; then
            :
        else
            ln "$primary" "$tmp_alias" || fail "failed to create alias: $alias"
        fi
        mv -f "$tmp_alias" "$BIN_DIR/$alias" || fail "failed to install alias atomically: $alias"
    done

    manifest_tmp=$BIN_DIR/.llmup-install-manifest.$$
    {
        printf 'managed-by=llmup-install\n'
        printf 'primary=llm-universal-proxy\n'
        printf 'aliases=llmup-config llmup-codex llmup-claude\n'
    } > "$manifest_tmp" || fail "failed to write install manifest"
    mv -f "$manifest_tmp" "$BIN_DIR/.llmup-install-manifest" || fail "failed to install manifest atomically"
}

next_command() {
    name=$1
    if path_contains "$BIN_DIR"; then
        printf '%s\n' "$name"
    else
        printf '%s/%s\n' "$BIN_DIR" "$name"
    fi
}

print_next_steps() {
    log "Installed llm-universal-proxy to $BIN_DIR"
    if ! path_contains "$BIN_DIR"; then
        log ""
        log "Your current shell PATH does not include $BIN_DIR."
        if [ "$MODIFY_PATH" -eq 1 ]; then
            log "After you reopen your terminal, the short commands should work."
        else
            log "Add $BIN_DIR to PATH to use the short commands."
        fi
        log "Use these absolute paths in this terminal:"
    else
        log ""
        log "Next:"
    fi

    config_cmd=$(next_command llmup-config)
    codex_cmd=$(next_command llmup-codex)
    claude_cmd=$(next_command llmup-claude)
    proxy_cmd=$(next_command llm-universal-proxy)

    log "  $config_cmd        set up model service"
    log "  $codex_cmd         start Codex CLI"
    log "  $claude_cmd        start Claude Code"
    log "  $proxy_cmd --help  advanced server usage"
}

detect_platform
base_url=$(release_base_url)
archive_url=$base_url/$asset_name
sha_url=$archive_url.sha256

TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/llmup-install.XXXXXX") || fail "failed to create temporary directory"
archive=$TMP_DIR/$asset_name
sha_file=$TMP_DIR/$asset_name.sha256
entries_file=$TMP_DIR/archive-entries.txt
extract_dir=$TMP_DIR/extract
mkdir -p "$extract_dir" || fail "failed to create extraction directory"

if [ -n "${detected_os:-}" ]; then
    log "Detected WSL; using Linux archive."
fi

log "Downloading $asset_name"
download_file "$archive_url" "$archive"
download_file "$sha_url" "$sha_file"

expected_sha=$(read_expected_sha256 "$sha_file")
actual_sha=$(sha256_of_file "$archive")
if [ "$expected_sha" != "$actual_sha" ]; then
    fail "checksum mismatch for $asset_name"
fi

validate_archive "$archive" "$entries_file"
tar -xzf "$archive" -C "$extract_dir" || fail "failed to extract archive"
[ -f "$extract_dir/llm-universal-proxy" ] || fail "archive did not contain llm-universal-proxy"

install_binary_and_aliases "$extract_dir"

if [ "$MODIFY_PATH" -eq 1 ] && ! path_contains "$BIN_DIR"; then
    ensure_profile_path
fi

print_next_steps
