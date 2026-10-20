#!/usr/bin/env bash
# Vendor the phone app's view library: Preact, its hooks, htm, and Preact Signals.
#
# The app has no build step, so these ship as the packages publish them, byte for byte, and the
# browser resolves their bare imports (`from "preact"`) through the page's import map. Each file's
# SHA-256 goes into `manifest.json`, and a test in `luxe-remote` holds the embedded files to it, so
# an edit to a vendored file fails the build instead of shipping as "Preact 11.0.1".
#
# To update: change a version below, run this, then check the app still boots. The same versions go
# in crates/luxe-remote/app-tests/package.json, whose type declarations check the app; a test there
# holds the two together.
#
# Requires: npm (only to fetch; nothing is installed). Usage: scripts/remote-vendor.sh

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="${repo_root}/crates/luxe-remote/app/vendor"
work_dir="$(mktemp -d)"
trap 'rm -rf "${work_dir}"' EXIT

# package@version, the file inside the package, and the name it is served under
files=(
	"preact@11.0.1 dist/preact.mjs preact.mjs"
	"preact@11.0.1 hooks/dist/hooks.mjs hooks.mjs"
	"htm@3.1.1 dist/htm.mjs htm.mjs"
	"@preact/signals-core@1.14.4 dist/signals-core.mjs signals-core.mjs"
	"@preact/signals@2.11.3 dist/signals.mjs signals.mjs"
)

fetch() {
	local spec="$1" dir="${work_dir}/$1"
	if [[ ! -d "${dir}" ]]; then
		mkdir -p "${dir}"
		(cd "${dir}" && npm pack --silent "${spec}" >/dev/null && tar xzf ./*.tgz)
	fi
	echo "${dir}/package"
}

rm -rf "${out_dir}"
mkdir -p "${out_dir}/licenses"
manifest="["
for line in "${files[@]}"; do
	read -r spec inner served <<<"${line}"
	package_dir="$(fetch "${spec}")"
	cp "${package_dir}/${inner}" "${out_dir}/${served}"
	name="${spec%@*}"
	slug="${name#@}"
	cp "${package_dir}/LICENSE" "${out_dir}/licenses/${slug//\//-}.txt"
	license="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["license"])' "${package_dir}/package.json")"
	sha="$(sha256sum "${out_dir}/${served}" | cut -d' ' -f1)"
	manifest+="
  {\"file\": \"${served}\", \"package\": \"${name}\", \"version\": \"${spec##*@}\", \"from\": \"${inner}\", \"license\": \"${license}\", \"sha256\": \"${sha}\"},"
done
printf '%s\n]\n' "${manifest%,}" >"${out_dir}/manifest.json"
echo "vendored ${#files[@]} files into ${out_dir#"${repo_root}/"}"
