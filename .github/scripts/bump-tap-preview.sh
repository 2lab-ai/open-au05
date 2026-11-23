#!/usr/bin/env bash
# Publish-time Homebrew tap bump for the preview channel.
#
# Renders Formula/open-au05-preview.rb from the assets the publish job just
# uploaded (the same sha256s brew will download) and pushes it to the tap, so
# `brew upgrade open-au05-preview` sees this build immediately instead of
# waiting on the tap's cron.
#
# Fail-closed: once invoked, the bump is not finished until the tap points at
# this build. Every abort except "the tap already carries this or a newer
# build" exits non-zero so the job goes red.
#
# Auth: GH_TOKEN (repo secret TAP_DISPATCH_TOKEN) through `gh auth setup-git`.
set -euo pipefail
export LC_ALL=C

fail() {
  echo "error: $*" >&2
  exit 1
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

render() {
  sed -e "s/@VERSION@/$VERSION/g" \
    -e "s/@TAG@/$TAG/g" \
    -e "s/@SHA_MACOS_AARCH64@/$SHA_MACOS_AARCH64/" \
    -e "s/@SHA_MACOS_X86_64@/$SHA_MACOS_X86_64/" \
    -e "s/@SHA_LINUX_AARCH64@/$SHA_LINUX_AARCH64/" \
    -e "s/@SHA_LINUX_X86_64@/$SHA_LINUX_X86_64/" \
    "$TAP_DIR/Formula/open-au05-preview.rb.tmpl" > "$TAP_DIR/Formula/open-au05-preview.rb"
  ruby -c "$TAP_DIR/Formula/open-au05-preview.rb" > /dev/null ||
    fail "rendered formula is not valid ruby"
  if grep -q "@[A-Z0-9_]\+@" "$TAP_DIR/Formula/open-au05-preview.rb"; then
    fail "unrendered @TOKEN@ left in the tap formula; template contract drifted"
  fi
}

main() {
  local TAG="${TAG:-}" BUILD_ID="${BUILD_ID:-}"
  local RELEASE_DIR="${RELEASE_DIR:-release}"
  local TAP_REPO="${TAP_REPO:-2lab-ai/homebrew-tap}"
  local TAP_REMOTE="${TAP_REMOTE:-https://github.com/${TAP_REPO}.git}"
  local TAP_BRANCH="${TAP_BRANCH:-master}"
  local TAP_DIR="${TAP_DIR:-tap-bump}"

  [ -n "$TAG" ] || fail "TAG is required"
  [ -n "$BUILD_ID" ] || fail "BUILD_ID is required"
  [ -n "${GH_TOKEN:-}" ] ||
    fail "GH_TOKEN is empty; register the tap push credential (secret TAP_DISPATCH_TOKEN)"
  command -v gh > /dev/null 2>&1 || fail "gh is not installed"

  [[ "$BUILD_ID" =~ ^([0-9]{4})-([0-9]{2})-([0-9]{2})-([0-9]{4})-[0-9a-f]{12}$ ]] ||
    fail "unexpected build id: $BUILD_ID (want YYYY-MM-DD-HHMM-<sha12>)"
  local VERSION="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.${BASH_REMATCH[4]}"
  [ "$TAG" = "preview-$BUILD_ID" ] ||
    fail "TAG $TAG does not match BUILD_ID $BUILD_ID (want preview-$BUILD_ID)"

  if [ -e "$TAP_DIR" ]; then
    fail "clone destination $TAP_DIR already exists; refusing to touch it"
  fi

  local name
  for name in open-au05-macos-aarch64 open-au05-macos-x86_64 open-au05-linux-aarch64 open-au05-linux-x86_64; do
    [ -f "$RELEASE_DIR/$name" ] || fail "missing published asset $RELEASE_DIR/$name"
  done
  local SHA_MACOS_AARCH64 SHA_MACOS_X86_64 SHA_LINUX_AARCH64 SHA_LINUX_X86_64
  SHA_MACOS_AARCH64="$(sha256_of "$RELEASE_DIR/open-au05-macos-aarch64")"
  SHA_MACOS_X86_64="$(sha256_of "$RELEASE_DIR/open-au05-macos-x86_64")"
  SHA_LINUX_AARCH64="$(sha256_of "$RELEASE_DIR/open-au05-linux-aarch64")"
  SHA_LINUX_X86_64="$(sha256_of "$RELEASE_DIR/open-au05-linux-x86_64")"

  gh auth setup-git --hostname github.com ||
    fail "gh auth setup-git failed; the tap credential is unusable"

  git clone --depth 1 --branch "$TAP_BRANCH" "$TAP_REMOTE" "$TAP_DIR" ||
    fail "cannot clone $TAP_REPO; check the tap credential's push access"
  git -C "$TAP_DIR" config user.name "github-actions[bot]"
  git -C "$TAP_DIR" config user.email "41898282+github-actions[bot]@users.noreply.github.com"

  local attempt CURRENT CUR_MIN TAG_MIN
  for attempt in 1 2 3; do
    git -C "$TAP_DIR" fetch origin "$TAP_BRANCH"
    git -C "$TAP_DIR" reset --hard "origin/$TAP_BRANCH"
    CURRENT="$(sed -n 's|^.*releases/download/\(preview-[^/]*\)/.*|\1|p' \
      "$TAP_DIR/Formula/open-au05-preview.rb" 2> /dev/null | head -1 || true)"
    if [ -n "$CURRENT" ] && [ "$CURRENT" != "$TAG" ]; then
      CUR_MIN="${CURRENT%-*}"
      TAG_MIN="${TAG%-*}"
      if [[ "$CUR_MIN" > "$TAG_MIN" ]]; then
        echo "tap already at newer $CURRENT; skipping"
        exit 0
      fi
      if [ "$CUR_MIN" = "$TAG_MIN" ]; then
        fail "tap at $CURRENT from the same minute as $TAG; order unknowable — rerun this job once the ordering is clear"
      fi
    fi
    render
    git -C "$TAP_DIR" add Formula/open-au05-preview.rb
    if git -C "$TAP_DIR" diff --cached --quiet; then
      echo "tap already current at $TAG"
      exit 0
    fi
    git -C "$TAP_DIR" commit -m "open-au05-preview $TAG"
    if git -C "$TAP_DIR" push origin "HEAD:$TAP_BRANCH"; then
      echo "tap bumped to $TAG"
      exit 0
    fi
    echo "push lost a race (attempt $attempt); refetching" >&2
  done
  fail "tap push failed after 3 attempts; preview $TAG is published but brew users cannot see it"
}

main "$@"
