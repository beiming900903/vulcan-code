#!/bin/sh
# Runs the installed setup program, or downloads @@VERSION@@ and checks its SHA-256 on a fresh install.
# 运行现有安装器；首次安装时下载 @@VERSION@@ 并校验其 SHA-256。
#
#   curl -fsSL https://static.openvulcan.com/vulcancode/install.sh | sh
#   curl -fsSL https://static.openvulcan.com/vulcancode/install.sh | sh -s -- --no-gui --yes
#
# `--help` lists the setup program's arguments. scripts/entry-scripts.mjs fills this template.
# `--help` 列出安装器参数。scripts/entry-scripts.mjs 填充本模板。

set -eu

Version='@@VERSION@@'
Origins='@@ORIGINS@@'

fail() {
    printf 'vulcan-code install: %s\n' "$*" >&2
    exit 1
}

# Under `curl | sh`, use the terminal instead of passing the script stream to the installer.
# 在 `curl | sh` 下使用终端，避免把脚本输入流交给安装器。
run_setup() {
    if [ -t 0 ]; then
        "$Program" "$@"
    elif (exec < /dev/tty) 2> /dev/null; then
        "$Program" "$@" < /dev/tty
    else
        "$Program" "$@" < /dev/null
    fi
}

case "$(uname -s)" in
    Linux) Os=linux ;;
    Darwin) Os=macos ;;
    *) fail "unsupported operating system $(uname -s); on Windows use install.ps1" ;;
esac

# Locate the same root as paths::default_root and read the route used for a fresh download.
# 按 paths::default_root 定位安装目录，并读取首次下载安装器时使用的线路。
Root=
case "$Os:${XDG_DATA_HOME:-}" in
    linux:/*) Root="$XDG_DATA_HOME/vulcan-code" ;;
    linux:*) if [ -n "${HOME:-}" ]; then Root="$HOME/.local/share/vulcan-code"; fi ;;
    macos:*) if [ -n "${HOME:-}" ]; then Root="$HOME/Library/Application Support/VulcanCode"; fi ;;
esac
case "${1:-install}" in
    install|update|-*) AutoUpdate=1 ;;
    *) AutoUpdate=0 ;;
esac
ValueFor=
# An unset route uses the published order for the bootstrap download.
# 未指定线路时，按发布时定义的顺序下载安装器。
Route=auto
for Arg do
    if [ -n "$ValueFor" ]; then
        case "$ValueFor" in
            --prefix) Root="$Arg" ;;
            --route) Route="$Arg" ;;
        esac
        ValueFor=
        continue
    fi
    case "$Arg" in
        --prefix=*) Root=${Arg#--prefix=} ;;
        --route=*) Route=${Arg#--route=} ;;
        --prefix|--mode|--port-range|--with|--channel|--route) ValueFor="$Arg" ;;
        --self-update|--no-self-update) AutoUpdate=0 ;;
    esac
done
if [ "$ValueFor" = --route ]; then fail '--route needs a value'; fi
case "$Route" in
    auto) ;;
    international) Origins=${Origins%% *} ;;
    china) Origins=${Origins#* } ;;
    *) fail '--route accepts auto, international or china' ;;
esac
if [ -n "$Root" ] && [ -z "$ValueFor" ]; then
    Program="$Root/setup/vulcan-code-setup"
    if [ ! -L "$Root" ] && [ ! -L "$Root/setup" ] && [ -f "$Root/setup/installation.json" ] &&
        [ ! -L "$Root/setup/installation.json" ] && [ -f "$Program" ] && [ -x "$Program" ] && [ ! -L "$Program" ]; then
        printf 'Starting the installed Vulcan Code setup program\n' >&2
        # Older installed programs need the explicit switch to update without asking.
        # 旧安装器需要显式开关才能自动更新。
        if [ "$AutoUpdate" = 1 ]; then set -- "$@" --self-update; fi
        run_setup "$@"
        exit "$?"
    fi
fi

Machine=$(uname -m)
case "$Machine" in
    x86_64|amd64) Arch=x64 ;;
    aarch64|arm64) Arch=arm64 ;;
    *) fail "unsupported architecture $Machine; Vulcan Code is built for x64 and arm64" ;;
esac
# A Rosetta shell reports x86_64 on Apple silicon; the native arm64 build is the right one there.
# Rosetta 下的 shell 在 Apple 芯片上报告 x86_64；此时应安装原生 arm64 版本。
if [ "$Os" = macos ] && [ "$Arch" = x64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
    Arch=arm64
fi
Platform="$Os-$Arch"
case "$Platform" in
    linux-x64) Digest='@@SHA256_LINUX_X64@@' ;;
    linux-arm64) Digest='@@SHA256_LINUX_ARM64@@' ;;
    macos-x64) Digest='@@SHA256_MACOS_X64@@' ;;
    macos-arm64) Digest='@@SHA256_MACOS_ARM64@@' ;;
esac
File="vulcan-code-setup-$Version-$Platform"

# A terminal shows a progress bar. An origin slower than 1 KB/s for 30 seconds fails, so the next one is tried
# instead of waiting without end.
# 终端上显示进度条。下载源连续 30 秒低于 1 KB/s 即失败，改试下一个，而不是无限等待。
if command -v curl > /dev/null 2>&1; then
    if [ -t 2 ]; then Progress=--progress-bar; else Progress=--silent; fi
    fetch() { curl -fL "$Progress" -S --connect-timeout 10 --speed-limit 1024 --speed-time 30 -o "$2" "$1"; }
elif command -v wget > /dev/null 2>&1; then
    fetch() { wget -q -T 30 -O "$2" "$1"; }
else
    fail "curl or wget is needed to download the setup program"
fi
if command -v sha256sum > /dev/null 2>&1; then
    sha256_of() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum > /dev/null 2>&1; then
    sha256_of() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
    fail "sha256sum or shasum is needed to check the setup program"
fi

Work=$(mktemp -d "${TMPDIR:-/tmp}/vulcan-code-setup.XXXXXX") || fail "cannot create a temporary directory"
trap 'rm -rf "$Work"' EXIT
trap 'exit 130' INT TERM
Program="$Work/vulcan-code-setup"

# The origins are tried in order; the mirror serves the same verified objects.
# 按顺序尝试各下载源；镜像提供相同的已校验对象。
Fetched=0
for Origin in $Origins; do
    printf 'Downloading the Vulcan Code setup program %s (%s) from %s\n' "$Version" "$Platform" "$Origin" >&2
    if fetch "$Origin/setup/$Version/$File" "$Program"; then
        Fetched=1
        break
    fi
    if [ "$Route" = auto ]; then
        printf 'vulcan-code install: cannot download from %s, trying the next origin\n' "$Origin" >&2
    fi
done
[ "$Fetched" = 1 ] || fail "cannot download $File"
Actual=$(sha256_of "$Program")
[ "$Actual" = "$Digest" ] || fail "$File does not match its published SHA-256 (expected $Digest, got $Actual)"
chmod 755 "$Program"
printf 'Starting the setup program %s\n' "$Version" >&2

run_setup "$@"
