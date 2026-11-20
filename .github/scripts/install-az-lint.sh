#!/usr/bin/env bash
set -euo pipefail

tools_dir="$RUNNER_TEMP/az-lint-tools"
bin_dir="$tools_dir/bin"
script_dir=$(dirname -- "${BASH_SOURCE[0]}")

if [[ "${1:-}" == --check ]]; then
    status=0
    for tool in ruff yamllint sqlfluff gitleaks actionlint shellcheck shfmt; do
        "$bin_dir/$tool" --version || status=1
    done
    exit "$status"
fi

python3 -m venv "$tools_dir"
"$bin_dir/python" -m pip --isolated install --disable-pip-version-check \
    --only-binary=:all: --require-hashes --no-deps \
    -r "$script_dir/az-lint-requirements.txt"

download() {
    local url=$1 checksum=$2 file=$3
    curl --fail --silent --show-error --location --retry 3 "$url" -o "$file"
    printf '%s  %s\n' "$checksum" "$file" | sha256sum --check --status
}

releases=https://github.com
download "$releases/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_x64.tar.gz" \
    551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb \
    "$tools_dir/gitleaks.tar.gz"
tar -xzf "$tools_dir/gitleaks.tar.gz" -C "$bin_dir" gitleaks

download "$releases/rhysd/actionlint/releases/download/v1.7.12/actionlint_1.7.12_linux_amd64.tar.gz" \
    8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8 \
    "$tools_dir/actionlint.tar.gz"
tar -xzf "$tools_dir/actionlint.tar.gz" -C "$bin_dir" actionlint

download "$releases/koalaman/shellcheck/releases/download/v0.11.0/shellcheck-v0.11.0.linux.x86_64.tar.xz" \
    8c3be12b05d5c177a04c29e3c78ce89ac86f1595681cab149b65b97c4e227198 \
    "$tools_dir/shellcheck.tar.xz"
tar -xf "$tools_dir/shellcheck.tar.xz" -C "$bin_dir" --strip-components=1 \
    shellcheck-v0.11.0/shellcheck

download "$releases/mvdan/sh/releases/download/v3.14.1/shfmt_v3.14.1_linux_amd64" \
    76e77641faa025814b77f153b29796b8e6fa2fca03e0c76a691608b86c7ea7bf "$bin_dir/shfmt"
chmod +x "$bin_dir/shfmt"
