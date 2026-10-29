#!/usr/bin/env bash
# Build static figma-agent-linux-<version>-<arch>.tar.gz release archives.
# The binaries are fully static (musl), so they run on any Linux distro,
# glibc or musl, with nothing else installed.
#
#   ./build.sh                  # x86_64 and aarch64
#   ./build.sh x86_64           # one architecture
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
ARCHES=("$@")
[ ${#ARCHES[@]} -gt 0 ] || ARCHES=(x86_64 aarch64)

mkdir -p "$ROOT/dist"
for arch in "${ARCHES[@]}"; do
    # Prefer musl; for the host architecture a static glibc build works too
    # (the agent does no DNS or user lookups, the parts of glibc that need shared libraries).
    target="$arch-unknown-linux-musl"
    flags=""
    if ! rustup target list --installed | grep -qx "$target"; then
        if [ "$arch" = "$(uname -m)" ]; then
            target="$arch-unknown-linux-gnu"
            flags="-C target-feature=+crt-static"
        else
            echo "missing Rust target: rustup target add $target" >&2
            exit 1
        fi
    fi
    RUSTFLAGS="$flags" cargo build --release --locked --target "$target" --manifest-path "$ROOT/Cargo.toml"
    if ! file -L "$ROOT/target/$target/release/figma-agent" | grep -q 'static'; then
        echo "$target build is not statically linked" >&2
        exit 1
    fi

    name="figma-agent-linux-$VERSION-$arch"
    stage="$ROOT/dist/$name"
    rm -rf "$stage" && mkdir -p "$stage"
    install -m 755 "$ROOT/target/$target/release/figma-agent" "$stage/figma-agent"
    install -m 755 "$ROOT"/packaging/{install.sh,uninstall.sh} "$stage/"
    install -m 644 "$ROOT/README.md" "$ROOT/LICENSE" "$stage/"

    tar -C "$ROOT/dist" -czf "$ROOT/dist/$name.tar.gz" --owner=0 --group=0 "$name"
    (cd "$ROOT/dist" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
    echo "built dist/$name.tar.gz ($(du -h "$ROOT/dist/$name.tar.gz" | cut -f1))"
done
