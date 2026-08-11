<p align="center"><strong>Myra CLI</strong> is a coding agent that runs locally on your computer.</p>

---

## Quickstart

### Installing and running Myra CLI

```shell
npm install -g @myralith/myra
```

Then run `myra` to get started.

The right native payload for your machine is selected automatically: the package
declares one optional dependency per platform, and npm installs only the one
matching your operating system and architecture.

### Supported platforms

| Platform | Architecture |
| --- | --- |
| Linux (musl, statically linked) | x64, arm64 |
| macOS | x64, arm64 |
| Windows | x64, arm64 |

### Signing in

Run `myra` and follow the sign-in prompt. See
[**Authentication**](./docs/authentication.md) for the available methods and how
credentials are stored.

## Embedding Myra

The TypeScript SDK spawns the CLI and exchanges JSONL events with it over
stdin/stdout:

```shell
npm install @myralith/myra-sdk
```

## Docs

- [**Getting started**](./docs/getting-started.md)
- [**Configuration**](./docs/config.md)
- [**Installing & building**](./docs/install.md)
- [**Non-interactive use**](./docs/exec.md)
- [**Contributing**](./docs/contributing.md)

## About this project

Myra CLI is derived from [OpenAI Codex CLI](https://github.com/openai/codex) and
carries its Apache-2.0 licence and attribution; see [NOTICE](NOTICE). It is not
affiliated with or endorsed by OpenAI, and it does not install or update
anything from OpenAI's distribution channels.

This repository is licensed under the [Apache-2.0 License](LICENSE).
