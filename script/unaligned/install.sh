#!/usr/bin/env sh
# Installs the Linux release archive into ~/.local, for the current user only:
#
#   ~/.local/<APP_ID>.app                                 the application
#   ~/.local/bin/<APP_ID>                                 link to its command
#   ~/.local/share/applications/<REVERSE_DNS_ID>.desktop  the menu entry
#
# Nothing else is written and nothing needs root. Another editor installed the
# same way (under its own name in ~/.local) is not touched.
#
# Usual use, from the copy attached to a release:
#   curl -fsSL https://github.com/<owner>/<repo>/releases/latest/download/install.sh | sh
#
# Environment:
#   IDE_VERSION           a release tag such as v0.1.0 (default: the latest release)
#   IDE_BUNDLE_PATH       install this local archive instead of downloading one
#                         (what script/unaligned/bundle-linux wrote); it is checked
#                         against the SHA256SUMS file beside it when there is one
#   IDE_RELEASE_BASE_URL  download <archive> and SHA256SUMS from this address
#                         instead of the GitHub release (a mirror, or a test)
set -eu

# Brand values. script/unaligned/bundle-linux fills these four lines in the copy
# it writes next to the archive. In the repository they stay empty and the
# values are read from crates/branding/brand.env.
PRODUCT_NAME=""
APP_ID=""
REVERSE_DNS_ID=""
RELEASES_REPOSITORY=""

temp=""

cleanup() {
    if [ -n "$temp" ] && [ -d "$temp" ]; then
        rm -rf "$temp"
    fi
    if [ -n "${staging:-}" ] && [ -d "$staging" ]; then
        rm -rf "$staging"
    fi
}

fail() {
    echo "install.sh: $*" >&2
    exit 1
}

# Prints the value of one KEY="value" line of the brand file.
brand_value() {
    sed -n "s/^$2=\"\\([^\"]*\\)\"\$/\\1/p" "$1" | head -n 1
}

load_brand() {
    if [ -n "$APP_ID" ]; then
        return
    fi
    brand_file=""
    case "$0" in
        */*) brand_file="$(dirname "$0")/../../crates/branding/brand.env" ;;
    esac
    if [ -z "$brand_file" ] || [ ! -f "$brand_file" ]; then
        fail "this copy has no product values. Use the install.sh attached to a release, or run script/unaligned/install.sh from a checkout of the repository."
    fi
    PRODUCT_NAME="$(brand_value "$brand_file" PRODUCT_NAME)"
    APP_ID="$(brand_value "$brand_file" APP_ID)"
    REVERSE_DNS_ID="$(brand_value "$brand_file" REVERSE_DNS_ID)"
    RELEASES_REPOSITORY="$(brand_value "$brand_file" RELEASES_REPOSITORY)"
}

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fL --proto '=https,file' -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "$2" "$1"
    else
        fail "neither curl nor wget was found"
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        fail "neither sha256sum nor shasum was found: the download cannot be verified"
    fi
}

# verify <archive file> <SHA256SUMS file> <archive name>
verify() {
    expected="$(awk -v name="$3" '$2 == name || $2 == "*" name { print $1; exit }' "$2")"
    if [ -z "$expected" ]; then
        fail "SHA256SUMS has no line for $3"
    fi
    actual="$(sha256_of "$1")"
    if [ "$expected" != "$actual" ]; then
        fail "checksum mismatch for $3 (expected $expected, got $actual). Nothing was installed."
    fi
    echo "Checksum verified: $actual"
}

main() {
    load_brand
    if [ -z "$PRODUCT_NAME" ] || [ -z "$APP_ID" ] || [ -z "$REVERSE_DNS_ID" ]; then
        fail "PRODUCT_NAME, APP_ID and REVERSE_DNS_ID must not be empty"
    fi
    if [ "$APP_ID" = "zed" ]; then
        fail "APP_ID is \"zed\": installing would replace an installed Zed"
    fi

    if [ "$(uname -s)" != "Linux" ]; then
        fail "this script installs the Linux build of $PRODUCT_NAME; this system is $(uname -s)"
    fi
    case "$(uname -m)" in
        x86_64 | amd64) arch="x86_64" ;;
        aarch64 | arm64) arch="aarch64" ;;
        *) fail "unsupported architecture $(uname -m)" ;;
    esac
    if [ -z "${HOME:-}" ] || [ ! -d "$HOME" ]; then
        fail "HOME is not set to a directory"
    fi

    archive="$APP_ID-linux-$arch.tar.gz"
    app_name="$APP_ID.app"
    version="${IDE_VERSION:-latest}"

    if [ -n "${TMPDIR:-}" ] && [ -d "${TMPDIR}" ]; then
        temp="$(mktemp -d "$TMPDIR/$APP_ID-XXXXXX")"
    else
        temp="$(mktemp -d "/tmp/$APP_ID-XXXXXX")"
    fi
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    if [ -n "${IDE_BUNDLE_PATH:-}" ]; then
        if [ ! -f "$IDE_BUNDLE_PATH" ]; then
            fail "IDE_BUNDLE_PATH is not a file: $IDE_BUNDLE_PATH"
        fi
        cp "$IDE_BUNDLE_PATH" "$temp/$archive"
        sums="$(dirname "$IDE_BUNDLE_PATH")/SHA256SUMS"
        if [ -f "$sums" ]; then
            verify "$temp/$archive" "$sums" "$archive"
        else
            echo "No SHA256SUMS beside the archive: installing it unverified."
        fi
    else
        if [ -n "${IDE_RELEASE_BASE_URL:-}" ]; then
            base="${IDE_RELEASE_BASE_URL%/}"
        elif [ -z "$RELEASES_REPOSITORY" ]; then
            fail "$PRODUCT_NAME has no releases page yet (RELEASES_REPOSITORY is empty in crates/branding/brand.env), so there is nothing to download. To install an archive you built yourself: IDE_BUNDLE_PATH=target/release/$archive script/unaligned/install.sh"
        elif [ "$version" = "latest" ]; then
            base="https://github.com/$RELEASES_REPOSITORY/releases/latest/download"
        else
            base="https://github.com/$RELEASES_REPOSITORY/releases/download/$version"
        fi
        echo "Downloading $PRODUCT_NAME ($version, $arch) from $base"
        fetch "$base/$archive" "$temp/$archive"
        fetch "$base/SHA256SUMS" "$temp/SHA256SUMS"
        verify "$temp/$archive" "$temp/SHA256SUMS" "$archive"
    fi

    # The archive must hold the application directory and nothing else.
    listing="$temp/listing"
    tar -tzf "$temp/$archive" >"$listing" || fail "the archive cannot be read"
    if [ ! -s "$listing" ]; then
        fail "the archive is empty"
    fi
    if grep -v -e "^$app_name/" -e "^$app_name\$" "$listing" >/dev/null; then
        fail "the archive holds files outside $app_name/"
    fi
    if grep -e '\.\./' -e '/\.\.$' "$listing" >/dev/null; then
        fail "the archive holds a path with '..'"
    fi

    # Unpack beside the destination, then swap: a failure before the swap
    # leaves the installed version as it was.
    mkdir -p "$HOME/.local"
    staging="$(mktemp -d "$HOME/.local/.$APP_ID-install-XXXXXX")"
    tar -xzf "$temp/$archive" -C "$staging"
    app="$HOME/.local/$app_name"
    cli="$app/bin/$APP_ID"
    if [ ! -x "$staging/$app_name/bin/$APP_ID" ]; then
        fail "the archive has no bin/$APP_ID"
    fi
    rm -rf "$app"
    mv "$staging/$app_name" "$app"
    rmdir "$staging"

    editor="$app/libexec/$APP_ID-editor"
    if [ -f "$editor" ] && command -v ldd >/dev/null 2>&1; then
        missing="$(ldd "$editor" 2>/dev/null | sed -n 's/^[[:space:]]*\(.*\) => not found$/\1/p')"
        if [ -n "$missing" ]; then
            echo "Warning: your system is missing libraries that $PRODUCT_NAME needs:"
            echo "$missing" | sed 's/^/    /'
            echo "Install them with your package manager, or $PRODUCT_NAME will fail to start."
        fi
    fi

    mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"
    ln -sf "$cli" "$HOME/.local/bin/$APP_ID"

    # The menu entry, with absolute paths so that it works when ~/.local/bin is
    # not in the desktop session's PATH.
    desktop_source="$app/share/applications/$REVERSE_DNS_ID.desktop"
    desktop_target="$HOME/.local/share/applications/$REVERSE_DNS_ID.desktop"
    icon="$app/share/icons/hicolor/512x512/apps/$APP_ID.png"
    if [ ! -f "$desktop_source" ]; then
        fail "the archive has no share/applications/$REVERSE_DNS_ID.desktop"
    fi
    case "$cli" in
        *[!A-Za-z0-9_./@+-]*) exec_cli="\"$cli\"" ;;
        *) exec_cli="$cli" ;;
    esac
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
            "Icon=$APP_ID") printf 'Icon=%s\n' "$icon" ;;
            "TryExec=$APP_ID") printf 'TryExec=%s\n' "$cli" ;;
            "Exec=$APP_ID") printf 'Exec=%s\n' "$exec_cli" ;;
            "Exec=$APP_ID "*) printf 'Exec=%s %s\n' "$exec_cli" "${line#"Exec=$APP_ID "}" ;;
            *) printf '%s\n' "$line" ;;
        esac
    done <"$desktop_source" >"$temp/entry.desktop"
    cp "$temp/entry.desktop" "$desktop_target"
    chmod 644 "$desktop_target"
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$HOME/.local/share/applications" >/dev/null 2>&1 || true
    fi

    if [ "$(command -v "$APP_ID" 2>/dev/null || true)" = "$HOME/.local/bin/$APP_ID" ]; then
        echo "$PRODUCT_NAME has been installed. Run it with '$APP_ID'."
    else
        echo "$PRODUCT_NAME has been installed in $app."
        echo "To run it from a terminal, add ~/.local/bin to your PATH:"
        case "${SHELL:-}" in
            *zsh)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.zshrc"
                echo "   source ~/.zshrc"
                ;;
            *fish)
                echo "   fish_add_path -U $HOME/.local/bin"
                ;;
            *)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.bashrc"
                echo "   source ~/.bashrc"
                ;;
        esac
        echo "To run it now: ~/.local/bin/$APP_ID"
    fi
}

main "$@"
