#!/usr/bin/env bash
set -euo pipefail

tools_dir="$RUNNER_TEMP/az-lint-tools"
bin_dir="$tools_dir/bin"

if [[ "${1:-}" == --check ]]; then
    status=0
    for tool in ruff yamllint sqlfluff semgrep gitleaks actionlint shellcheck shfmt; do
        "$bin_dir/$tool" --version || status=1
    done
    exit "$status"
fi

python3 -m venv "$tools_dir"
"$bin_dir/python" -m pip install --disable-pip-version-check \
    --only-binary=:all: \
    "https://files.pythonhosted.org/packages/f0/20/fecedea2209f574ab92033b344e01a7f0c2dc76c6d6d19a14c9f8141173f/ruff-0.17.3-py3-none-manylinux_2_17_x86_64.manylinux2014_x86_64.whl#sha256=7fa4b0f931cdbcb8a52044f370a3eb429fb42eb88807b0e800fcaa98dd42b24e" \
    "https://files.pythonhosted.org/packages/05/92/aed08e68de6e6a3d7c2328ce7388072cd6affc26e2917197430b646aed02/yamllint-1.38.0-py3-none-any.whl#sha256=fc394a5b3be980a4062607b8fdddc0843f4fa394152b6da21722f5d59013c220" \
    "https://files.pythonhosted.org/packages/f3/de/78c2f6d023a2dada127baf9a41a7186707d500de89947dd7259d33700bb2/sqlfluff-4.5.0-py3-none-any.whl#sha256=471a9abcad8061e36b69f711a6d89393241e30f9d1e0869f79965c0ae2c889c7" \
    "https://files.pythonhosted.org/packages/c5/fd/85ac31a5ec53feee54ebdce933ab1c94af81695a2b244b0baefdbcdba92a/semgrep-1.183.0-cp310.cp311.cp312.cp313.cp314.py310.py311.py312.py313.py314-none-manylinux_2_34_x86_64.whl#sha256=f36115c4d0cb43056ceb308273421a2f6a35e055e7e029601e130850f836646e"

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
