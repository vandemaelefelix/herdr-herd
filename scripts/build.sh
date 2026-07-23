#!/bin/sh
# herdr [[build]] step: build the release binary.
# Source ~/.cargo/env so cargo is found even when herdr launches without
# ~/.cargo/bin on PATH (GUI / login-less launch). The [ -f ] guard means a
# missing env file can't abort the build.
set -e
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
exec cargo build --release
