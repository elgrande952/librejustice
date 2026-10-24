#!/usr/bin/env bash
set -Eeuo pipefail
exec bash "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)/sol-fable-production/runtime/run-integration.sh"
