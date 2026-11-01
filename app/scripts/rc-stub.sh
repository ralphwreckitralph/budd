#!/bin/sh
# A stand-in for llvm-rc, for `cargo check --target x86_64-pc-windows-msvc` on a Mac without LLVM.
# embed-resource probes it with `/?` and expects an LLVM "Resource Converter"; for a compile it only
# needs the `/fo` output to exist, since a check build never links. Never use it for a real build.
case " $* " in
  *" /? "*|*" -V "*)
    echo "LLVM Resource Converter stub (no-preprocess)"
    exit 0 ;;
esac
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    /fo) out="$2"; shift ;;
  esac
  shift
done
[ -n "$out" ] && : > "$out"
exit 0
