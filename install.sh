#!/bin/sh
# Install ncc under ~/.local:
#   ~/.local/bin/ncc            the compiler
#   $NC_HOME/std/               the standard library  (imported as "std/...")
#   $NC_HOME/libc/              the libc bindings     (imported as "libc/...")
# where the NC home, $NC_HOME, defaults to $XDG_DATA_HOME/nc, which defaults to
# ~/.local/share/nc.
set -eu

repo=$(cd "$(dirname "$0")" && pwd)
: "${HOME:?HOME is not set}"
bin="$HOME/.local/bin"
if [ -n "${NC_HOME:-}" ]; then
	home=$NC_HOME
else
	case "${XDG_DATA_HOME:-}" in
	/*) home="$XDG_DATA_HOME/nc" ;;
	*) home="$HOME/.local/share/nc" ;;
	esac
fi
mkdir -p "$home"
home=$(cd "$home" && pwd)

echo "Building ncc (release)..."
cargo build --release --manifest-path "$repo/Cargo.toml"

mkdir -p "$bin"
install -m 755 "$repo/target/release/ncc" "$bin/ncc"
echo "Installed $bin/ncc"

# Replace the installed libraries with the repo's copies so removed modules don't linger.
for lib in std libc; do
	rm -rf "$home/$lib"
	mkdir -p "$home/$lib"
	if [ -d "$repo/$lib" ]; then
		cp -R "$repo/$lib/." "$home/$lib/"
	fi
	echo "Installed $home/$lib/"
done

case ":$PATH:" in
*":$bin:"*) ;;
*)
	echo
	echo "Add ncc to your PATH by adding this line to your shell profile:"
	echo "  export PATH=\"$bin:\$PATH\""
	;;
esac

if [ -n "${NC_HOME:-}" ]; then
	echo
	echo "ncc finds std/ and libc/ through \$NC_HOME, so keep it exported in your shell profile:"
	echo "  export NC_HOME=\"$home\""
fi
