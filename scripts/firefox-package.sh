#!/usr/bin/env bash
# Firefox packaging for RustyPub.
#
# Chrome loads extension/ directly (service_worker-only manifest).
# Firefox needs background.scripts and gecko identity from
# extension/manifest.firefox.json.
#
# Commands:
#   stage [dir]  Persistent tree for about:debugging (default:
#                <repo>/target/extension-firefox). Only replaces a dir under
#                <repo>/target/ that we previously staged (marker file).
#   lint         Disposable stage → Mozilla lint → clean up
#   build        Disposable stage → lint → ZIP under web-ext-artifacts/ → clean up
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$script_dir/.." && pwd)"
src="$root/extension"
artifacts="$root/web-ext-artifacts"
default_stage="$root/target/extension-firefox"
marker=".rustypub-firefox-stage"
web_ext=(npx --yes web-ext@10.5.0)

die() {
  echo "error: $*" >&2
  exit 1
}

# Resolve to an absolute path without requiring the path to exist.
abs_path() {
  local path="$1"
  if [[ "$path" = /* ]]; then
    printf '%s\n' "$path"
  else
    printf '%s\n' "$(pwd)/$path"
  fi
}

# Only allow persistent staging under <repo>/target/.
assert_stage_dest_allowed() {
  local dest="$1"
  local target_root="$root/target"
  case "$dest" in
    "$target_root" | "$target_root"/*) ;;
    *)
      die "stage destination must be under $target_root (got $dest)"
      ;;
  esac
}

# Replace only missing paths, our previous stage (marker), or the well-known
# default stage path under target/. Never recursive-delete an arbitrary
# existing directory that lacks the marker.
assert_safe_to_replace() {
  local dest="$1"
  if [[ ! -e "$dest" ]]; then
    return 0
  fi
  if [[ -f "$dest/$marker" ]]; then
    return 0
  fi
  if [[ "$dest" == "$default_stage" ]]; then
    return 0
  fi
  die "refusing to replace $dest (not a RustyPub Firefox stage; missing $marker)"
}

populate_stage() {
  local dest="$1"
  mkdir -p "$dest"
  local path name
  for path in "$src"/*; do
    name="$(basename "$path")"
    case "$name" in
      manifest.json | manifest.firefox.json | prepare-firefox.sh) continue ;;
    esac
    cp -a "$path" "$dest/"
  done
  cp "$src/manifest.firefox.json" "$dest/manifest.json"
  # Marker for safe re-stage; ignored by web-ext if someone packages this tree.
  : >"$dest/$marker"
  printf '%s\n' "$marker" >"$dest/.web-extignore"
}

stage_persistent() {
  local dest
  dest="$(abs_path "${1:-$default_stage}")"
  # Normalize trailing slash
  dest="${dest%/}"
  assert_stage_dest_allowed "$dest"
  assert_safe_to_replace "$dest"
  rm -rf -- "$dest"
  populate_stage "$dest"
  echo "Firefox extension ready: $dest"
  echo "Load temporary add-on: $dest/manifest.json"
}

with_disposable_stage() {
  local stage_dir
  stage_dir="$(mktemp -d "${TMPDIR:-/tmp}/rustypub-firefox.XXXXXX")"
  cleanup() {
    rm -rf -- "$stage_dir"
  }
  trap cleanup EXIT
  populate_stage "$stage_dir"
  "$@" "$stage_dir"
  trap - EXIT
  cleanup
}

cmd_lint() {
  local stage_dir="$1"
  "${web_ext[@]}" lint --source-dir "$stage_dir"
}

cmd_build() {
  local stage_dir="$1"
  "${web_ext[@]}" lint --source-dir "$stage_dir"
  mkdir -p "$artifacts"
  "${web_ext[@]}" build \
    --source-dir "$stage_dir" \
    --artifacts-dir "$artifacts" \
    --overwrite-dest
  echo "Package written under $artifacts/"
}

usage() {
  cat <<EOF
Usage: bash scripts/firefox-package.sh <command>

  stage [dir]   Persistent Firefox tree (default: target/extension-firefox)
  lint          Run Mozilla web-ext lint on a disposable stage
  build         Lint then build ZIP into web-ext-artifacts/ (default)
EOF
}

main() {
  local cmd="${1:-build}"
  shift || true
  case "$cmd" in
    stage)
      stage_persistent "${1:-}"
      ;;
    lint)
      with_disposable_stage cmd_lint
      ;;
    build)
      with_disposable_stage cmd_build
      ;;
    -h | --help | help)
      usage
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
}

main "$@"
