# Third-party notices

`seo-geo-skill` bundles and adapts work from other projects. This file records
what came from where and under which licence. Nothing here is optional
courtesy — MIT and the Creative Commons licences below all require attribution
to travel with the material.

## Skill and agent documentation

The skill definitions in `skills/` and the subagent definitions in `agents/`
are derived from two MIT-licensed projects. The prose has been substantially
rewritten (every execution step now targets the `seogeo` binary instead of
Python scripts), but the skill taxonomy, scoring methodologies, and much of the
domain guidance originate upstream.

| Source | Author | Licence | What we use |
|--------|--------|---------|-------------|
| [AgriciDaniel/claude-seo](https://github.com/AgriciDaniel/claude-seo) | agricidaniel | MIT | The `seo-*` skill family, the SEO subagents, the reference library, schema templates |
| [zubair-trabzada/geo-seo-claude](https://github.com/zubair-trabzada/geo-seo-claude) | Zubair Trabzada | MIT | The `geo-*` skill family, the GEO subagents, the citability and brand-authority scoring models, the report templates |

Several upstream skills were themselves community contributions, integrated
into `claude-seo` with their authors' permission. Their attribution carries
forward here:

| Skill | Original author | Original repository |
|-------|-----------------|---------------------|
| `seo-cluster` | Lutfiya Miller | [Drfiya/semantic-cluster-engine](https://github.com/Drfiya/semantic-cluster-engine) |
| `seo-sxo` | Florian Schmitz | [tools-enerix/claude-sxo-skill](https://github.com/tools-enerix/claude-sxo-skill) |
| `seo-drift` | Dan Colta | [dancolta/seo-drift-monitor](https://github.com/dancolta/seo-drift-monitor) |
| `seo-hreflang` (cultural profiles) | Chris Muller | [Chriss54/claude-blog-multilingual](https://github.com/Chriss54/claude-blog-multilingual) |
| `seo-content-brief` | puneetindersingh | community contribution |

## Content and data

| Material | Licence | Where it appears |
|----------|---------|------------------|
| LLM-typical phrasing catalogue, from the Wikipedia "AI Cleanup" project | CC BY-SA 4.0 | `AI_PATTERNS` and the replacement table in `src/cmd/content.rs`, used by `seogeo content-quality` and `seogeo content-humanize` |
| FLOW prompt library | CC BY 4.0 | Fetched on demand by `seogeo sync-flow`; every synced file gets an attribution header naming the source and licence |
| Google Search ranking-update timeline | Facts, not creative expression; every entry cites a Google-owned URL | `data/google-updates.json`, served by `seogeo seo-updates` |

CC BY-SA 4.0 is a copyleft licence. The phrase lists derived from it are
marked in the source and remain under CC BY-SA 4.0; the surrounding code is
MIT.

## Rust dependencies

The binary links the crates listed in `Cargo.toml`, each under its own licence
(predominantly MIT or Apache-2.0). To produce a full dependency licence report:

```bash
cargo install cargo-about
cargo about generate about.hbs
```

## Trademarks

Google, Search Console, PageSpeed Insights, Chrome UX Report, GA4, YouTube,
Bing, Moz, Ahrefs, DataForSEO, Perplexity, Anthropic, Claude, OpenAI, ChatGPT,
Gemini, and Cloudflare are trademarks of their respective owners. This project
is not affiliated with, endorsed by, or sponsored by any of them. Product names
are used only to identify the services these commands talk to.
