#!/bin/bash
# Keyfire — one-shot macOS dev environment setup (Mac port Phase 2).
# Run on the test MacBook:
#   curl -fsSL https://raw.githubusercontent.com/Trigr-it/trigr-tauri/main/scripts/mac-dev-setup.sh | bash
# Idempotent: safe to re-run; every step skips itself if already done.
# Interactive moments: macOS password for Homebrew, a GUI dialog for the
# Xcode Command Line Tools, and `gh auth login` + `claude` sign-in at the end.

set -e

REPO_DIR="$HOME/Development/trigr-tauri"

step() { printf '\n\033[1;33m== %s ==\033[0m\n' "$1"; }

step "1/8 Xcode Command Line Tools"
if xcode-select -p >/dev/null 2>&1; then
  echo "Already installed."
else
  xcode-select --install || true
  echo ""
  echo ">>> A macOS dialog has opened. Click Install and wait for it to finish,"
  echo ">>> then RE-RUN this script. Exiting for now."
  exit 0
fi

step "2/8 Homebrew"
if command -v brew >/dev/null 2>&1; then
  echo "Already installed."
else
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
fi
# Apple Silicon brew lives in /opt/homebrew and needs PATH set up once.
if [ -x /opt/homebrew/bin/brew ]; then
  eval "$(/opt/homebrew/bin/brew shellenv)"
  grep -q 'brew shellenv' "$HOME/.zprofile" 2>/dev/null || \
    echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> "$HOME/.zprofile"
fi

step "3/8 Node 22 + GitHub CLI"
brew list node@22 >/dev/null 2>&1 || brew install node@22
brew list gh >/dev/null 2>&1 || brew install gh
brew link --overwrite node@22 >/dev/null 2>&1 || true

step "4/8 Rust (rustup)"
if command -v rustc >/dev/null 2>&1 || [ -x "$HOME/.cargo/bin/rustc" ]; then
  echo "Already installed."
else
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi
source "$HOME/.cargo/env"

step "5/8 Tauri CLI (cargo tauri) — compiles once, several minutes"
if command -v cargo-tauri >/dev/null 2>&1; then
  echo "Already installed."
else
  cargo install tauri-cli --version '^2' --locked
fi

step "6/8 Claude Code"
if command -v claude >/dev/null 2>&1; then
  echo "Already installed."
else
  curl -fsSL https://claude.ai/install.sh | bash
fi

step "7/8 Clone repo + npm install"
if [ -d "$REPO_DIR/.git" ]; then
  echo "Repo already at $REPO_DIR"
else
  mkdir -p "$(dirname "$REPO_DIR")"
  git clone https://github.com/Trigr-it/trigr-tauri.git "$REPO_DIR"
fi
cd "$REPO_DIR"
git config user.name "Trigr"
git config user.email "admin@usetrigr.com"
npm install

step "8/8 Done — remaining manual steps"
cat <<'EOF'

Setup complete. Three interactive steps remain:

  1. gh auth login          (pick GitHub.com > HTTPS > browser login;
                             gives this Mac push access to the repo)
  2. claude                 (run inside ~/Development/trigr-tauri;
                             sign in on first launch)
  3. First dev run:         cd ~/Development/trigr-tauri && cargo tauri dev

When the engine work starts, macOS will prompt for Accessibility and
Input Monitoring permissions (System Settings > Privacy & Security).
Grant both to the app/terminal when asked - global hotkeys cannot work
without them.
EOF
