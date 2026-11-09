#!/bin/sh
# Builds the release binary, places it in the plugin root, where the
# manifest's commands expect it, and copies it to the plugin's config
# directory, where the claude shell function and the status line run it.
set -eu

cd "$(dirname "$0")/.."

if ! command -v cargo >/dev/null 2>&1; then
  for dir in "$HOME/.cargo/bin" "$HOME/.homebrew/bin" /opt/homebrew/bin /usr/local/bin; do
    if [ -x "$dir/cargo" ]; then
      PATH="$dir:$PATH"
      export PATH
      break
    fi
  done
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found: install Rust (https://rustup.rs or 'brew install rust')" >&2
  exit 1
fi

cargo build --release --locked
cp target/release/herdr-multiaccount ./herdr-multiaccount.new
mv -f ./herdr-multiaccount.new ./herdr-multiaccount
./herdr-multiaccount install-bin || echo "could not copy the binary for the shell function; opening the list does it" >&2
echo "built $(pwd)/herdr-multiaccount"
