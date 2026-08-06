# Rebrand to Myra Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebrand NPM packages, CLI entrypoint binary launcher, packaging tooling, release workflows, and user documentation from `@openai/codex` / `codex` to `@myralith/myra` / `myra`.

**Architecture:** Update NPM `package.json` package names under `@myralith/` scope, update the NodeJS binary launcher script to `bin/myra.js` pointing to `@myralith/myra-<platform>` platform dependencies and `myra` binary, update `build_npm_package.py` staging definitions, and update CI release manifests and docs. Internal Rust crate names remain `codex-*`.

**Tech Stack:** Node.js, Python, pnpm, bash, GitHub Actions YAML.

## Global Constraints
- Primary NPM CLI Package: `@myralith/myra`
- TypeScript SDK Package: `@myralith/myra-sdk`
- Proxy Package: `@myralith/myra-responses-api-proxy`
- Native Platform Packages: `@myralith/myra-<os>-<cpu>`
- CLI Entrypoint File: `codex-cli/bin/myra.js`
- CLI Binary Name in package.json `bin`: `myra`

---

### Task 1: Rebrand CLI Package & Entrypoint (`codex-cli`)

**Files:**
- Modify: `codex-cli/package.json`
- Rename & Modify: `codex-cli/bin/codex.js` -> `codex-cli/bin/myra.js`

**Interfaces:**
- Consumes: None
- Produces: `@myralith/myra` package metadata and `myra` binary entrypoint launcher.

- [ ] **Step 1: Update `codex-cli/package.json`**

Update package name, binary name, description, and repository URL:
```json
{
  "name": "@myralith/myra",
  "version": "0.0.0-dev",
  "description": "Myra CLI is a coding agent that runs locally on your computer.",
  "license": "Apache-2.0",
  "bin": {
    "myra": "bin/myra.js"
  },
  "type": "module",
  "engines": {
    "node": ">=16"
  },
  "files": [
    "bin/myra.js"
  ],
  "repository": {
    "type": "git",
    "url": "git+https://github.com/myralith/myra.git",
    "directory": "codex-cli"
  }
}
```

- [ ] **Step 2: Rename `bin/codex.js` to `bin/myra.js` and update references**

Rename file using git mv or fs rename and update platform package mappings:
```javascript
const PLATFORM_PACKAGE_BY_TARGET = {
  "x86_64-unknown-linux-musl": "@myralith/myra-linux-x64",
  "aarch64-unknown-linux-musl": "@myralith/myra-linux-arm64",
  "x86_64-apple-darwin": "@myralith/myra-darwin-x64",
  "aarch64-apple-darwin": "@myralith/myra-darwin-arm64",
  "x86_64-pc-windows-msvc": "@myralith/myra-win32-x64",
  "aarch64-pc-windows-msvc": "@myralith/myra-win32-arm64",
};
```
Update executable lookup name inside `findCodexExecutable`:
`process.platform === "win32" ? "myra.exe" : "myra"` (with fallback to `codex.exe`/`codex` if missing).
Update update command string:
`npm install -g @myralith/myra@latest`, `pnpm add -g @myralith/myra@latest`, `bun install -g @myralith/myra@latest`.
Update `isPnpmOwnedCodexInstall`: check `@myralith/myra`.
Update environment variables: `MYRA_MANAGED_PACKAGE_ROOT`, `MYRA_MANAGED_BY_NPM/PNPM/BUN`.

- [ ] **Step 3: Test execution of `bin/myra.js`**

Run: `node codex-cli/bin/myra.js --help` (or verify error message if vendor binary is not staged).
Expected: Clean execution or expected missing binary error referencing `@myralith/myra`.

- [ ] **Step 4: Commit changes**

```bash
git add codex-cli/package.json codex-cli/bin/
git commit -m "rebrand(cli): update package name to @myralith/myra and entrypoint to bin/myra.js"
```

---

### Task 2: Rebrand TypeScript SDK & Proxy Package Metadata

**Files:**
- Modify: `sdk/typescript/package.json`
- Modify: `codex-rs/responses-api-proxy/npm/package.json`
- Rename & Modify: `codex-rs/responses-api-proxy/npm/bin/codex-responses-api-proxy.js` -> `codex-rs/responses-api-proxy/npm/bin/myra-responses-api-proxy.js`

**Interfaces:**
- Consumes: `@myralith/myra` package name from Task 1
- Produces: `@myralith/myra-sdk` and `@myralith/myra-responses-api-proxy` packages.

- [ ] **Step 1: Update `sdk/typescript/package.json`**

Update `name` to `@myralith/myra-sdk`, description to "TypeScript SDK for Myra APIs.", and keywords to include `myralith` and `myra`.

- [ ] **Step 2: Update `codex-rs/responses-api-proxy/npm/package.json` and launcher**

Update package `name` to `@myralith/myra-responses-api-proxy`, update `bin` map to `{ "myra-responses-api-proxy": "bin/myra-responses-api-proxy.js" }`.
Rename `bin/codex-responses-api-proxy.js` to `bin/myra-responses-api-proxy.js`.

- [ ] **Step 3: Commit changes**

```bash
git add sdk/typescript/package.json codex-rs/responses-api-proxy/npm/
git commit -m "rebrand(sdk): update sdk and proxy packages to @myralith scope"
```

---

### Task 3: Update NPM Build & Staging Script (`build_npm_package.py`)

**Files:**
- Modify: `codex-cli/scripts/build_npm_package.py`

**Interfaces:**
- Consumes: Rebranded package names from Tasks 1 & 2
- Produces: Staging script supporting `@myralith/myra` packaging.

- [ ] **Step 1: Update package constants in `build_npm_package.py`**

Update `CODEX_NPM_NAME`:
```python
CODEX_NPM_NAME = "@myralith/myra"
```
Update `CODEX_PLATFORM_PACKAGES`:
```python
CODEX_PLATFORM_PACKAGES: dict[str, dict[str, str]] = {
    "codex-linux-x64": {
        "npm_name": "@myralith/myra-linux-x64",
        "npm_tag": "linux-x64",
        "target_triple": "x86_64-unknown-linux-musl",
        "os": "linux",
        "cpu": "x64",
    },
    "codex-linux-arm64": {
        "npm_name": "@myralith/myra-linux-arm64",
        "npm_tag": "linux-arm64",
        "target_triple": "aarch64-unknown-linux-musl",
        "os": "linux",
        "cpu": "arm64",
    },
    "codex-darwin-x64": {
        "npm_name": "@myralith/myra-darwin-x64",
        "npm_tag": "darwin-x64",
        "target_triple": "x86_64-apple-darwin",
        "os": "darwin",
        "cpu": "x64",
    },
    "codex-darwin-arm64": {
        "npm_name": "@myralith/myra-darwin-arm64",
        "npm_tag": "darwin-arm64",
        "target_triple": "aarch64-apple-darwin",
        "os": "darwin",
        "cpu": "arm64",
    },
    "codex-win32-x64": {
        "npm_name": "@myralith/myra-win32-x64",
        "npm_tag": "win32-x64",
        "target_triple": "x86_64-pc-windows-msvc",
        "os": "win32",
        "cpu": "x64",
    },
    "codex-win32-arm64": {
        "npm_name": "@myralith/myra-win32-arm64",
        "npm_tag": "win32-arm64",
        "target_triple": "aarch64-pc-windows-msvc",
        "os": "win32",
        "cpu": "arm64",
    },
}
```

- [ ] **Step 2: Update file staging paths in `stage_sources`**

Update `bin/codex.js` reference to `bin/myra.js` and `bin/codex-responses-api-proxy.js` to `bin/myra-responses-api-proxy.js`.
Update verification printed messages to reference `bin/myra.js` and `bin/myra-responses-api-proxy.js`.

- [ ] **Step 3: Test `build_npm_package.py` syntax**

Run: `python3 -m py_compile codex-cli/scripts/build_npm_package.py`
Expected: Clean compilation with 0 errors.

- [ ] **Step 4: Commit changes**

```bash
git add codex-cli/scripts/build_npm_package.py
git commit -m "rebrand(scripts): update build_npm_package.py to target @myralith/myra packages"
```

---

### Task 4: Update Workspaces, GitHub Workflows, Dev Container & Documentation

**Files:**
- Modify: `README.md`
- Modify: `.devcontainer/codex-install/package.json`
- Modify: `.devcontainer/Dockerfile.secure`
- Modify: `.github/workflows/rust-release.yml`
- Modify: `.github/ISSUE_TEMPLATE/3-cli.yml`
- Modify: `codex-cli/scripts/README.md`

**Interfaces:**
- Consumes: Final `@myralith/myra` package names and `bin/myra.js` path.
- Produces: Updated docs and release workflows matching the rebrand.

- [ ] **Step 1: Update README.md and issue templates**

In `README.md`, update `npm install -g @openai/codex` to `npm install -g @myralith/myra`.
In `.github/ISSUE_TEMPLATE/3-cli.yml`, update `https://npmjs.com/package/@openai/codex` to `https://npmjs.com/package/@myralith/myra`.
In `codex-cli/scripts/README.md`, update references to `@myralith/myra`.

- [ ] **Step 2: Update Dev Container & CI release workflows**

In `.devcontainer/codex-install/package.json`, update `"@openai/codex"` to `"@myralith/myra"`.
In `.devcontainer/Dockerfile.secure`, update `'@openai/codex'` check to `'@myralith/myra'`.
In `.github/workflows/rust-release.yml`, update comments and publish references from `@openai/codex` to `@myralith/myra`.

- [ ] **Step 3: Run formatting check**

Run: `pnpm run format` (or `npm run format`)
Expected: PASS

- [ ] **Step 4: Commit changes**

```bash
git add README.md .devcontainer/ .github/ codex-cli/scripts/README.md
git commit -m "rebrand(docs): update documentation and CI release workflows to @myralith/myra"
```
