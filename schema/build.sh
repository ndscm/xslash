#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

pnpm dlx ts-json-schema-generator --path '*.ts' --type '*' --out 'schema.json'
