# Design Specification: Rebrand to Myra (`@myralith/myra`)

## Overview
Rebrand the npm packages, binary launcher, build/staging scripts, release workflows, and user-facing documentation from `@openai/codex` (and `codex`) to `@myralith/myra` (and `myra`). Internal Rust crate names (`codex-*`) remain unchanged to preserve workspace build system stability and follow repository conventions.

## Target NPM Package Taxonomy

| Purpose | Old Package Name | New Package Name |
| :--- | :--- | :--- |
| **Main CLI Meta-package** | `@openai/codex` | `@myralith/myra` |
| **TypeScript SDK** | `@openai/codex-sdk` | `@myralith/myra-sdk` |
| **Responses API Proxy** | `@openai/codex-responses-api-proxy` | `@myralith/myra-responses-api-proxy` |
| **Linux x64 Binary** | `@openai/codex-linux-x64` | `@myralith/myra-linux-x64` |
| **Linux ARM64 Binary** | `@openai/codex-linux-arm64` | `@myralith/myra-linux-arm64` |
| **macOS x64 Binary** | `@openai/codex-darwin-x64` | `@myralith/myra-darwin-x64` |
| **macOS ARM64 Binary** | `@openai/codex-darwin-arm64` | `@myralith/myra-darwin-arm64` |
| **Windows x64 Binary** | `@openai/codex-win32-x64` | `@myralith/myra-win32-x64` |
| **Windows ARM64 Binary** | `@openai/codex-win32-arm64` | `@myralith/myra-win32-arm64` |

---

## Detailed Components

### 1. CLI Package (`codex-cli`)
- `codex-cli/package.json`:
  - `name`: `@myralith/myra`
  - `description`: "Myra CLI is a coding agent that runs locally on your computer."
  - `bin`: `{ "myra": "bin/myra.js" }`
  - `files`: `["bin/myra.js"]`
- `codex-cli/bin/myra.js` (renamed from `bin/codex.js`):
  - Target triple platform map updated to `@myralith/myra-<platform>`.
  - Binary filename lookup updated to `myra` / `myra.exe` (with fallback to `codex` / `codex.exe`).
  - Reinstall hints updated to use `@myralith/myra@latest`.
  - Environment variables set: `MYRA_MANAGED_PACKAGE_ROOT`, `MYRA_MANAGED_BY_*` (maintaining backward-compatible fallback for `CODEX_*`).

### 2. TypeScript SDK (`sdk/typescript`)
- `sdk/typescript/package.json`:
  - `name`: `@myralith/myra-sdk`
  - `description`: "TypeScript SDK for Myra APIs."
  - `keywords`: `["myralith", "myra", "sdk", "typescript", "api"]`

### 3. Responses API Proxy (`codex-rs/responses-api-proxy/npm`)
- `codex-rs/responses-api-proxy/npm/package.json`:
  - `name`: `@myralith/myra-responses-api-proxy`
  - `bin`: `{ "myra-responses-api-proxy": "bin/myra-responses-api-proxy.js" }`
- Entrypoint renamed to `bin/myra-responses-api-proxy.js`.

### 4. Staging & Release Tooling (`codex-cli/scripts/build_npm_package.py`)
- Update package naming constants:
  - `CODEX_NPM_NAME`: `"@myralith/myra"`
  - `CODEX_PLATFORM_PACKAGES`: Update all `npm_name` entries to `@myralith/myra-<platform>`.
- Update staging and verification messages for CLI, SDK, Proxy, and native payloads.

### 5. Workspaces, CI & Documentation
- Update `.devcontainer/codex-install/package.json` and `.devcontainer/Dockerfile.secure`.
- Update `.github/workflows/rust-release.yml` for publishing `@myralith/myra`.
- Update `.github/ISSUE_TEMPLATE/3-cli.yml` and `README.md`.

---

## Verification Plan
1. Test packaging staging: `python3 codex-cli/scripts/build_npm_package.py --package codex --version 0.0.0-test`
2. Test CLI binary launcher execution with `bin/myra.js --version` and `--help`.
3. Verify `package.json` schemas and `pnpm` workspace resolution.
