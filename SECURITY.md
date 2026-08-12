# Security policy

## Reporting a vulnerability

Report privately through
[GitHub Security Advisories](https://github.com/asale-ai/seo-geo-skill/security/advisories/new).
Please do not open a public issue for anything exploitable.

Include the version (`seogeo --version`), your platform, the exact command, and
the smallest input that reproduces the problem. We aim to acknowledge within
three working days and to ship a fix or a mitigation plan within fourteen.

## Threat model

`seogeo` fetches URLs an agent supplies, and an agent's input frequently comes
from a user, a page, or a search result. The URL is therefore untrusted, and
the primary risk is server-side request forgery: persuading the tool to fetch
something on the local network or a cloud metadata endpoint and hand the body
back to the model.

Secondary risks: leaking API keys into logs an agent will read, and executing
untrusted content that arrives inside a fetched page.

## What the tool does about it

**Every** outbound request goes through `src/safety.rs`. There is no bypass and
no "just this once" path.

| Control | Where | What it stops |
|---------|-------|---------------|
| Scheme allowlist (`http`, `https` only) | `validate_url` | `file://`, `gopher://`, `ftp://` reads |
| Hard-blocked hostnames | `BLOCKED_HOSTNAMES` | `localhost`, and every documented AWS / Azure / GCP / Oracle / Alibaba metadata endpoint, including the IPv6 form |
| FQDN-form normalisation | `normalize_hostname` | `metadata.google.internal.` slipping past an exact-match blocklist |
| `inet_aton` canonicalisation | `normalize_hostname` | `http://2130706433/`, `http://0x7f000001/`, `http://0177.0.0.1/` — decimal, hex, octal, and short-form encodings of a private address |
| Authority-confusion rejection | `reject_authority_confusion` | Userinfo (`https://public.test@127.0.0.1/`), backslashes, percent-encoding in the authority, and `#@` fragment/userinfo tricks that make two parsers disagree |
| Private / reserved range check | `is_safe_ip` | RFC 1918, loopback, link-local, multicast, CGNAT (100.64/10), benchmarking (198.18/15), IPv4-mapped IPv6, IPv6 unique-local and link-local |
| Resolver-level validation | `SafeResolver` | The same checks applied to **every** host resolved during a request — including redirect targets, so a 302 to an internal address fails at connect time rather than after the check |
| All-records rule | `validate_url_strict` | A hostname with one public and one private record. Every returned address must be public; one bad record fails the whole lookup, so an attacker cannot race the resolver |
| Response size cap | `RequestOptions::max_bytes` | Memory exhaustion from an unbounded body |
| Request deadline | `RequestOptions::timeout` | A slow-loris body that never ends |

Verify the policy yourself:

```bash
seogeo url-safety http://169.254.169.254/latest/meta-data/   # exit 2
seogeo url-safety http://2130706433/                          # exit 2
seogeo url-safety https://user:pass@example.com/              # exit 2
seogeo url-safety https://example.com/ --strict --json        # exit 0, prints the pinned IP
```

### Known limits

- **Headless-browser fetches.** `seogeo render` and `seogeo screenshot` drive
  Chrome, which does its own DNS resolution in its own process. The target URL
  is validated before Chrome launches, but subresource requests the page makes
  are outside our resolver. Treat rendering an untrusted URL as running that
  page's JavaScript, because that is what it is.
- **Proxies.** Setting `SEOGEO_PROXY` / `HTTPS_PROXY` routes traffic through
  the proxy, which resolves hostnames itself. URL validation still runs
  locally, so the policy above still applies to the requested URL — but the
  pinned-DNS guarantee does not extend through the proxy.
- **`--urls-file` and `--batch`.** Every URL in the file is validated
  individually. The file itself is trusted input; do not point these at a file
  an untrusted party controls without reading it first.

## Credentials

Credentials are never read from, or written to, the repository.

| Credential | Source |
|------------|--------|
| `GOOGLE_API_KEY` | env, or `~/.config/seogeo/google-api.json` |
| `GOOGLE_APPLICATION_CREDENTIALS` | env, or `service_account_path` in the same file |
| `MOZ_API_KEY`, `BING_WEBMASTER_API_KEY` | env, or `~/.config/seogeo/backlinks-api.json` |
| `DATAFORSEO_LOGIN` / `DATAFORSEO_PASSWORD` | env only |
| `INDEXNOW_KEY` | env, or `indexnow_key` in the Google config file |

Google API keys are stripped from error text before it is printed
(`google::redact`), because agents log stderr and those logs get pasted around.
Requests that carry a key send it in the `X-Goog-Api-Key` header rather than
the query string, so it does not land in server access logs either. The one
API that requires the key in the URL — Bing Webmaster — never has its URL
echoed back in an error.

If you believe a key leaked through this tool, rotate it first and report
second.

## Supply chain

- Release binaries are built by
  [`.github/workflows/release.yml`](.github/workflows/release.yml) on
  GitHub-hosted runners from a tagged commit.
- Every archive is listed in `SHA256SUMS`, published with the release.
  `install.sh` and `install.ps1` verify the checksum before installing and
  abort without touching an existing binary if it does not match.
- Verify a download by hand:

  ```bash
  shasum -a 256 -c SHA256SUMS --ignore-missing
  ```

## Supported versions

Only the latest release receives security fixes.
