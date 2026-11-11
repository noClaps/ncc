#!/bin/sh
# Install ncc into the NC home ($NC_HOME, defaulting to ~/.nc):
#   $NC_HOME/bin/ncc   the compiler
#   $NC_HOME/std/      the standard library  (imported as "std/...")
#   $NC_HOME/libc/     the libc bindings     (imported as "libc/...")
set -eu

repo=$(cd "$(dirname "$0")" && pwd)
if [ -n "${NC_HOME:-}" ]; then
	home=$NC_HOME
else
	home="${HOME:?neither NC_HOME nor HOME is set}/.nc"
fi
mkdir -p "$home"
home=$(cd "$home" && pwd)

echo "Building ncc (release)..."
cargo build --release --manifest-path "$repo/Cargo.toml"

mkdir -p "$home/bin"
install -m 755 "$repo/target/release/ncc" "$home/bin/ncc"
echo "Installed $home/bin/ncc"

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
*":$home/bin:"*) ;;
*)
	echo
	echo "Add ncc to your PATH by adding this line to your shell profile:"
	echo "  export PATH=\"$home/bin:\$PATH\""
	;;
esac

if [ -n "${NC_HOME:-}" ]; then
	echo
	echo "ncc finds std/ and libc/ through \$NC_HOME, so keep it exported in your shell profile:"
	echo "  export NC_HOME=\"$home\""
fi
