#!/bin/sh
# Type-checks the app for Windows from a Mac or Linux box: `cargo check` only, no link. tauri-build
# compiles a Windows resource on the way, so point it at llvm-rc when LLVM is installed, else at the
# stub, and give cc a C preprocessor for the .rc file.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
if command -v llvm-rc >/dev/null 2>&1; then
  export RC=llvm-rc
else
  export RC="$here/rc-stub.sh"
fi
: "${CC_x86_64_pc_windows_msvc:=clang}"
export CC_x86_64_pc_windows_msvc
rustup target add x86_64-pc-windows-msvc >/dev/null
exec cargo check --target x86_64-pc-windows-msvc -p budd-app "$@"
