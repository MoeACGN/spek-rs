#!/bin/bash
# Rust's windows-gnullvm target links -lmsvcrt. In llvm-mingw that import
# library pulls api-ms-win-crt-*.dll, including the private CRT set that
# fails to load on some Windows 10 systems (0xc000007b). libucrtbase.a
# resolves the same symbols directly from ucrtbase.dll, which ships with
# Windows 10.
real=/opt/toolchains/llvm-mingw-20261020-ucrt-ubuntu-22.04-x86_64/bin/x86_64-w64-mingw32-clang
args=()
for a in "$@"; do
  if [ "$a" = "-lmsvcrt" ]; then
    args+=("-lucrtbase")
  else
    args+=("$a")
  fi
done
exec "$real" "${args[@]}"
