#!/bin/bash
cd "$(dirname "$0")"
export PATH="$HOME/.local/bin:$PATH"
if ! command -v uv >/dev/null 2>&1; then
  echo "Installing uv, a small tool that sets up Python for Readalong. This happens once."
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export PATH="$HOME/.local/bin:$PATH"
fi
[ -x .venv/bin/python ] || echo "Setting up Readalong for the first time. This takes a minute or two..."
uv sync --python 3.12 --quiet || { echo "Setup didn't finish. Check your internet connection and try again."; read -r; exit 1; }
exec .venv/bin/python readalong.py "$@"
