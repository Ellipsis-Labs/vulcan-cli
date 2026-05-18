#!/bin/sh
set -eu

REPO="Ellipsis-Labs/vulcan-cli"
DEFAULT_VERSION="__VULCAN_VERSION__"
INSTALL_DIR="${VULCAN_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${VULCAN_VERSION:-$DEFAULT_VERSION}"
INSTALL_AGENT_SKILLS="${VULCAN_INSTALL_AGENT_SKILLS:-prompt}"
AGENT_TARGET="${VULCAN_AGENT_TARGET:-claude}"
AGENT_SCOPE="${VULCAN_AGENT_SCOPE:-user}"
FORCE_AGENT_SKILLS="${VULCAN_FORCE_AGENT_SKILLS:-no}"

usage() {
  cat <<EOF
Install vulcan from GitHub Releases.

Usage:
  install.sh [--version <version>] [--install-dir <dir>] [--with-agent-skills|--no-agent-skills]
             [--agent-target <target>] [--agent-scope <scope>] [--force-agent-skills]

Environment:
  VULCAN_VERSION                Version to install, with or without a leading v
  VULCAN_INSTALL_DIR            Install directory (default: \$HOME/.local/bin)
  VULCAN_INSTALL_AGENT_SKILLS   yes, no, or prompt (default: prompt)
  VULCAN_AGENT_TARGET           claude, cursor, codex, or agentskills (default: claude)
  VULCAN_AGENT_SCOPE            user or project (default: user)
  VULCAN_FORCE_AGENT_SKILLS     yes or no — overwrite locally modified skill files (default: no)
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      if [ "$#" -lt 2 ]; then
        echo "error: --version requires a value" >&2
        exit 1
      fi
      VERSION="$2"
      shift 2
      ;;
    --install-dir)
      if [ "$#" -lt 2 ]; then
        echo "error: --install-dir requires a value" >&2
        exit 1
      fi
      INSTALL_DIR="$2"
      shift 2
      ;;
    --with-agent-skills)
      INSTALL_AGENT_SKILLS="yes"
      shift
      ;;
    --no-agent-skills)
      INSTALL_AGENT_SKILLS="no"
      shift
      ;;
    --agent-target)
      if [ "$#" -lt 2 ]; then
        echo "error: --agent-target requires a value" >&2
        exit 1
      fi
      AGENT_TARGET="$2"
      shift 2
      ;;
    --agent-scope)
      if [ "$#" -lt 2 ]; then
        echo "error: --agent-scope requires a value" >&2
        exit 1
      fi
      AGENT_SCOPE="$2"
      shift 2
      ;;
    --force-agent-skills)
      FORCE_AGENT_SKILLS="yes"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

case "$AGENT_TARGET" in
  claude|cursor|codex|agentskills) ;;
  *)
    echo "error: unsupported --agent-target: $AGENT_TARGET" >&2
    echo "expected one of: claude, cursor, codex, agentskills" >&2
    exit 1
    ;;
esac

case "$AGENT_SCOPE" in
  user|project) ;;
  *)
    echo "error: unsupported --agent-scope: $AGENT_SCOPE" >&2
    echo "expected one of: user, project" >&2
    exit 1
    ;;
esac

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command not found: $1" >&2
    exit 1
  fi
}

need curl
need grep
need install
need mktemp
need sed
need tar
need uname

if command -v sha256sum >/dev/null 2>&1; then
  CHECKSUM_CMD="sha256sum -c -"
elif command -v shasum >/dev/null 2>&1; then
  CHECKSUM_CMD="shasum -a 256 -c -"
else
  echo "error: required command not found: sha256sum or shasum" >&2
  exit 1
fi

prompt_tty() {
  if [ ! -r /dev/tty ] || [ ! -w /dev/tty ]; then
    return 1
  fi
  printf '%s' "$1" > /dev/tty
  IFS= read -r REPLY < /dev/tty || return 1
  printf '%s\n' "$REPLY"
}

prompt_agent_install() {
  answer="$(prompt_tty "Install Vulcan Agent Skills now? [y/N] " || true)"
  case "$answer" in
    y|Y|yes|YES|Yes)
      target="$(prompt_tty "Agent target [claude/cursor/codex/agentskills] [$AGENT_TARGET]: " || true)"
      if [ -n "$target" ]; then
        AGENT_TARGET="$target"
      fi
      scope="$(prompt_tty "Install scope [user/project] [$AGENT_SCOPE]: " || true)"
      if [ -n "$scope" ]; then
        AGENT_SCOPE="$scope"
      fi
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

should_install_agent_skills() {
  case "$INSTALL_AGENT_SKILLS" in
    yes|true|1)
      return 0
      ;;
    no|false|0)
      return 1
      ;;
    prompt|"")
      prompt_agent_install
      return $?
      ;;
    *)
      echo "error: VULCAN_INSTALL_AGENT_SKILLS must be yes, no, or prompt" >&2
      exit 1
      ;;
  esac
}

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
    TARGET="x86_64-unknown-linux-musl"
    ;;
  Linux-aarch64|Linux-arm64)
    TARGET="aarch64-unknown-linux-musl"
    ;;
  Darwin-x86_64)
    TARGET="x86_64-apple-darwin"
    ;;
  Darwin-arm64)
    TARGET="aarch64-apple-darwin"
    ;;
  *)
    echo "error: unsupported platform: $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

if [ "$VERSION" = "__VULCAN_VERSION__" ] || [ -z "$VERSION" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p')"
  if [ -z "$VERSION" ]; then
    echo "error: could not determine latest vulcan release version" >&2
    exit 1
  fi
fi

VERSION="${VERSION#v}"
TAG="v$VERSION"
ASSET="vulcan-${VERSION}-${TARGET}.tar.gz"
BASE_URL="https://github.com/$REPO/releases/download/$TAG"

TMPDIR="$(mktemp -d)"
cleanup() {
  rm -rf "$TMPDIR"
}
trap cleanup EXIT INT TERM

echo "Installing vulcan $VERSION for $TARGET"
mkdir -p "$INSTALL_DIR"

curl -fsSLo "$TMPDIR/$ASSET" "$BASE_URL/$ASSET"
curl -fsSLo "$TMPDIR/vulcan-checksums-sha256.txt" "$BASE_URL/vulcan-checksums-sha256.txt"

CHECKSUM_LINE="$(grep "  $ASSET$" "$TMPDIR/vulcan-checksums-sha256.txt" || true)"
if [ -z "$CHECKSUM_LINE" ]; then
  echo "error: checksum not found for $ASSET" >&2
  exit 1
fi

(cd "$TMPDIR" && printf '%s\n' "$CHECKSUM_LINE" | $CHECKSUM_CMD)
tar -xzf "$TMPDIR/$ASSET" -C "$TMPDIR"
install -m 755 "$TMPDIR/vulcan" "$INSTALL_DIR/vulcan"

echo "Installed vulcan to $INSTALL_DIR/vulcan"
"$INSTALL_DIR/vulcan" version

if should_install_agent_skills; then
  case "$AGENT_TARGET" in
    claude|cursor|codex|agentskills) ;;
    *)
      echo "error: unsupported agent target: $AGENT_TARGET" >&2
      echo "expected one of: claude, cursor, codex, agentskills" >&2
      exit 1
      ;;
  esac
  case "$AGENT_SCOPE" in
    user|project) ;;
    *)
      echo "error: unsupported agent scope: $AGENT_SCOPE" >&2
      echo "expected one of: user, project" >&2
      exit 1
      ;;
  esac
  case "$FORCE_AGENT_SKILLS" in
    yes|true|1)
      "$INSTALL_DIR/vulcan" agent install --target "$AGENT_TARGET" --scope "$AGENT_SCOPE" --force
      ;;
    *)
      "$INSTALL_DIR/vulcan" agent install --target "$AGENT_TARGET" --scope "$AGENT_SCOPE"
      ;;
  esac
else
  echo "Skipped agent skill installation. Run 'vulcan agent install --target cursor --scope user' later."
fi
