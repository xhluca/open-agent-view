#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
website_dir="${repo_root}/website"
pages_repo="open-agent-view/open-agent-view.github.io"
pages_dir="${OAV_PAGES_CHECKOUT:-${repo_root}/../open-agent-view.github.io}"

for command in gh git npm rsync; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'missing required publication tool: %s\n' "$command" >&2
    exit 1
  }
done

if [[ ! -d "${pages_dir}/.git" ]]; then
  [[ ! -e "$pages_dir" ]] || {
    printf 'refusing to replace non-repository path: %s\n' "$pages_dir" >&2
    exit 1
  }
  gh repo clone "$pages_repo" "$pages_dir"
fi

remote="$(git -C "$pages_dir" remote get-url origin)"
case "$remote" in
  "https://github.com/${pages_repo}.git"|"git@github.com:${pages_repo}.git") ;;
  *)
    printf 'refusing unexpected Pages remote: %s\n' "$remote" >&2
    exit 1
    ;;
esac

npm --prefix "$website_dir" ci --no-audit
npm --prefix "$website_dir" audit --omit=dev --audit-level=high
npm --prefix "$website_dir" run lint
npm --prefix "$website_dir" test
npm --prefix "$website_dir" run test:visual
npm --prefix "$website_dir" run export

source_dir="${website_dir}/dist/static"
[[ -f "${source_dir}/index.html" && -f "${source_dir}/install.sh" && -f "${source_dir}/install.ps1" ]] || {
  printf 'static export is incomplete: %s\n' "$source_dir" >&2
  exit 1
}

rsync -a --delete --exclude .git/ "${source_dir}/" "${pages_dir}/"
git -C "$pages_dir" add -A

if git -C "$pages_dir" diff --cached --quiet; then
  printf 'open-agent-view.github.io is already current\n'
  exit 0
fi

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "${repo_root}/Cargo.toml" | head -n 1)"
git -C "$pages_dir" commit -m "site: publish Open Agent View ${version}"
git -C "$pages_dir" push origin HEAD:main
printf 'published https://open-agent-view.github.io/\n'
