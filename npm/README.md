# @asale/seogeo

The `seogeo` binary, delivered through npm.

```bash
npx @asale/seogeo --version
```

Audit a website for classic search, **measure** whether the answer engines
actually mention it, and read your own server logs to see what their crawlers
did — ChatGPT, Claude, Perplexity, Gemini, Google AI Overviews.

```bash
npx @asale/seogeo commands                   # every subcommand, and the skills that use it
npx @asale/seogeo citability https://example.com
npx @asale/seogeo logs /var/log/nginx/access.log
npx @asale/seogeo visibility providers
```

To install the agent skills as well as the binary:

```bash
npx @asale/seogeo install --target npx
```

## What this package contains

A launcher, not a binary. On install it downloads the prebuilt release build
for your platform from
[the GitHub release](https://github.com/asale-ai/seo-geo-skill/releases) and
verifies it against the published `SHA256SUMS`. The npm version always matches
the release tag it downloads, so `@asale/seogeo@0.2.0` can only ever fetch
`v0.2.0`.

If your machine skips lifecycle scripts (`--ignore-scripts`, some CI
sandboxes), the download is retried the first time you run the command.

Platforms: macOS and Linux on x86_64 and aarch64, Windows on x86_64. Linux
picks the musl build unless glibc is detected, because musl runs on both.

## Documentation

Everything — the skills, the MCP server, the answer-engine measurement, the
credentials each command needs — is in the
[main README](https://github.com/asale-ai/seo-geo-skill#readme).

MIT
