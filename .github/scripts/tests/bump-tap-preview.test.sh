#!/usr/bin/env bash
# Hermetic regression test for .github/scripts/bump-tap-preview.sh.
set -euo pipefail

SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/bump-tap-preview.sh"
[ -f "$SCRIPT" ] || {
  echo "missing script under test: $SCRIPT" >&2
  exit 1
}
command -v ruby > /dev/null 2>&1 || {
  echo "ruby is required (the script syntax-checks the rendered formula)" >&2
  exit 1
}

ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT

TOKEN="gho_sentinel_must_not_appear_000000000000"
TAG="preview-2026-11-23-0615-abc123def456"
VERSION="2026.11.23.0615"

FAILURES=0
ok() { printf 'ok   - %s\n' "$1"; }
notok() {
  printf 'FAIL - %s\n' "$1"
  FAILURES=$((FAILURES + 1))
}
expect_zero() {
  if [ "$RC" -eq 0 ]; then ok "$1"; else
    notok "$1 (rc=$RC)"
    printf '%s\n' "$OUT" | sed 's/^/       | /'
  fi
}
expect_nonzero() {
  if [ "$RC" -ne 0 ]; then ok "$1"; else
    notok "$1 (rc=0, expected failure)"
    printf '%s\n' "$OUT" | sed 's/^/       | /'
  fi
}
expect_out() {
  case "$OUT" in
    *"$1"*) ok "$2" ;;
    *)
      notok "$2 (output lacks '$1')"
      printf '%s\n' "$OUT" | sed 's/^/       | /'
      ;;
  esac
}
expect_true() { if "${@:2}"; then ok "$1"; else notok "$1"; fi; }
expect_absent() { if "${@:2}"; then notok "$1"; else ok "$1"; fi; }

mkdir -p "$ROOT/bin"
cat > "$ROOT/bin/gh" << 'MOCK'
#!/usr/bin/env bash
if [ "${1:-}" = "auth" ] && [ "${2:-}" = "setup-git" ]; then exit 0; fi
echo "unexpected gh invocation: $*" >&2
exit 1
MOCK
chmod +x "$ROOT/bin/gh"
export PATH="$ROOT/bin:$PATH"
export GIT_CONFIG_GLOBAL="$ROOT/gitconfig"
export GIT_CONFIG_NOSYSTEM=1
git config --global user.name "test"
git config --global user.email "test@example.com"
git config --global init.defaultBranch master

FORMULA_TMPL="$ROOT/Formula.tmpl"
cat > "$FORMULA_TMPL" << 'TMPL'
class OpenAu05Preview < Formula
  desc "test fixture mirroring the tap template"
  homepage "https://github.com/2lab-ai/open-au05"
  version "@VERSION@"

  on_macos do
    on_arm do
      url "https://github.com/2lab-ai/open-au05/releases/download/@TAG@/open-au05-macos-aarch64"
      sha256 "@SHA_MACOS_AARCH64@"
    end
    on_intel do
      url "https://github.com/2lab-ai/open-au05/releases/download/@TAG@/open-au05-macos-x86_64"
      sha256 "@SHA_MACOS_X86_64@"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/2lab-ai/open-au05/releases/download/@TAG@/open-au05-linux-aarch64"
      sha256 "@SHA_LINUX_AARCH64@"
    end
    on_intel do
      url "https://github.com/2lab-ai/open-au05/releases/download/@TAG@/open-au05-linux-x86_64"
      sha256 "@SHA_LINUX_X86_64@"
    end
  end
end
TMPL

sha256_of() {
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

seed_tap() {
  local origin="$1" current="${2:-}"
  local work="${origin%.git}-seed"
  git init --quiet --bare "$origin"
  git -C "$origin" symbolic-ref HEAD refs/heads/master
  git init --quiet "$work"
  mkdir -p "$work/Formula"
  cp "$FORMULA_TMPL" "$work/Formula/open-au05-preview.rb.tmpl"
  if [ -n "$current" ]; then
    local ver
    ver="$(echo "${current#preview-}" | awk -F- '{print $1"."$2"."$3"."$4}')"
    sed -e "s/@VERSION@/$ver/g" -e "s/@TAG@/$current/g" -e "s/@SHA[A-Z_0-9]*@/00seed/g" \
      "$FORMULA_TMPL" > "$work/Formula/open-au05-preview.rb"
  fi
  git -C "$work" add -A
  git -C "$work" commit --quiet -m "seed"
  git -C "$work" push --quiet "$origin" HEAD:master
}

make_release() {
  local dir="$1"
  mkdir -p "$dir"
  local name
  for name in open-au05-macos-aarch64 open-au05-macos-x86_64 open-au05-linux-aarch64 open-au05-linux-x86_64; do
    echo "$name binary for $TAG" > "$dir/$name"
  done
}

new_case() {
  CASE_DIR="$ROOT/case-$1"
  ORIGIN="$CASE_DIR/tap.git"
  WORK="$CASE_DIR/work"
  mkdir -p "$WORK"
  seed_tap "$ORIGIN" "${2:-}"
  make_release "$WORK/release"
}

RUN_N=0
run_bump() {
  local tag="${1:-$TAG}" token="${2-$TOKEN}"
  local build="${3-}"
  [ -n "$build" ] || build="${tag#preview-}"
  RUN_N=$((RUN_N + 1))
  TAP_CLONE="${TAP_DIR_FORCE:-tap-bump-$RUN_N}"
  set +e
  OUT="$(cd "$WORK" && TAG="$tag" BUILD_ID="$build" GH_TOKEN="$token" \
    RELEASE_DIR=release TAP_REMOTE="$ORIGIN" TAP_BRANCH=master TAP_DIR="$TAP_CLONE" \
    bash "$SCRIPT" 2>&1)"
  RC=$?
  set -e
}

pushed_formula() { git -C "$ORIGIN" show "master:Formula/open-au05-preview.rb"; }
sha_after_url() {
  awk -v want="/$2\"" '
    hit { if ($1 == "sha256") { gsub(/"/, "", $2); print $2 }; exit }
    $1 == "url" && index($0, want) { hit = 1 }
  ' <<< "$1"
}
commit_count() { git -C "$ORIGIN" rev-list --count master; }

new_case no-token
before="$(commit_count)"
run_bump "$TAG" ""
expect_nonzero "missing GH_TOKEN fails"
expect_out "TAP_DISPATCH_TOKEN" "error names TAP_DISPATCH_TOKEN"
expect_true "pushes nothing" [ "$before" = "$(commit_count)" ]

new_case no-binary
rm -f "$WORK/release/open-au05-linux-aarch64"
run_bump
expect_nonzero "missing binary fails"
expect_out "open-au05-linux-aarch64" "error names missing asset"

new_case happy "preview-2026-11-22-1200-000000000000"
run_bump
expect_zero "happy path succeeds"
expect_out "tap bumped to $TAG" "happy path reports push"
formula="$(pushed_formula)"
expect_true "formula carries version" grep -q "version \"$VERSION\"" <<< "$formula"
expect_true "formula carries tag" grep -q "download/$TAG/" <<< "$formula"
for asset in open-au05-macos-aarch64 open-au05-macos-x86_64 open-au05-linux-aarch64 open-au05-linux-x86_64; do
  expect_true "formula pairs $asset url with sha256" \
    [ "$(sha_after_url "$formula" "$asset")" = "$(sha256_of "$WORK/release/$asset")" ]
done
expect_absent "no unrendered token" grep -q "@[A-Z0-9_]\+@" <<< "$formula"
expect_absent "token never reaches log" grep -qF "$TOKEN" <<< "$OUT"
expect_absent "token never reaches clone" grep -rqF "$TOKEN" "$WORK/$TAP_CLONE/.git"

after_first="$(commit_count)"
run_bump
expect_zero "rerun succeeds as no-op"
expect_out "already current" "rerun detects current tag"
expect_true "rerun adds no commit" [ "$after_first" = "$(commit_count)" ]

new_case first-bump
run_bump
expect_zero "first bump into template-only tap succeeds"

new_case newer-tap "preview-2026-11-24-0100-ffffffffffff"
before="$(commit_count)"
run_bump
expect_zero "newer tap is left alone"
expect_true "newer tap not rolled back" [ "$before" = "$(commit_count)" ]

new_case drift
seed_work="$CASE_DIR/tap-seed"
printf '  sha256 "@SHA_UNEXPECTED_X86_64@"\n' >> "$seed_work/Formula/open-au05-preview.rb.tmpl"
git -C "$seed_work" commit --quiet -am "drift"
git -C "$seed_work" push --quiet "$ORIGIN" HEAD:master
run_bump
expect_nonzero "drifted template token fails"

echo
if [ "$FAILURES" -eq 0 ]; then
  echo "all tap-bump regression cases passed"
else
  echo "$FAILURES tap-bump regression case(s) failed"
  exit 1
fi
