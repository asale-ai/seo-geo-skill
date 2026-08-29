//! MCP server over stdio.
//!
//! The skills only reach agents that read `SKILL.md` files. MCP reaches the
//! rest — Cursor, Windsurf, Claude Desktop, n8n, anything that speaks the
//! protocol — from the same binary, with no extra runtime and no second
//! implementation of the checks.
//!
//! Each tool shells back into this executable with `--json` and returns the
//! output verbatim. Re-entering as a subprocess rather than calling
//! `dispatch` in-process is deliberate: every command in this binary prints
//! its result to stdout, and stdout here is the JSON-RPC transport.

use std::io::{BufRead, Write};
use std::process::ExitCode;

use serde_json::{json, Value};

use crate::output::CmdResult;

/// Protocol revision this server implements. When a client asks for a
/// different one its value is echoed back — the handshake is a negotiation,
/// and refusing an unknown revision breaks clients that are otherwise
/// compatible.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// How a tool argument reaches the CLI.
enum Arg {
    /// Bare value, in declaration order.
    Positional(&'static str),
    /// `--name value`
    Flag(&'static str),
    /// `--name value` repeated once per array element.
    Repeated(&'static str),
}

struct Tool {
    name: &'static str,
    /// Shown to the model. This is what decides whether the tool gets picked,
    /// so it states the outcome, not the mechanism.
    description: &'static str,
    command: &'static [&'static str],
    args: &'static [Arg],
    required: &'static [&'static str],
    /// JSON Schema `properties`, as a literal so no schema is built twice.
    schema: &'static str,
}

const URL_PROP: &str = r#"{"url":{"type":"string","description":"Page URL to analyse"}}"#;

const TOOLS: &[Tool] = &[
    Tool {
        name: "seo_page_audit",
        description:
            "Extract every SEO element from one page: title, meta description, canonical, \
                      headings, images, links, structured data, hreflang, and word count.",
        command: &["parse"],
        args: &[Arg::Flag("url")],
        required: &["url"],
        schema: URL_PROP,
    },
    Tool {
        name: "geo_citability",
        description: "Score how quotable a page is to an answer engine, passage by passage, with \
                      the weakest passages named so they can be rewritten.",
        command: &["citability"],
        args: &[Arg::Positional("url")],
        required: &["url"],
        schema: URL_PROP,
    },
    Tool {
        name: "geo_crawler_access",
        description: "Check whether each AI crawler is allowed by robots.txt AND actually served \
                      when it asks — permission and delivery are different failures.",
        command: &["robots"],
        args: &[Arg::Positional("url")],
        required: &["url"],
        schema: URL_PROP,
    },
    Tool {
        name: "geo_llms_txt",
        description: "Validate a site's llms.txt, or generate one from its sitemap.",
        command: &["llms-txt", "validate"],
        args: &[Arg::Positional("url")],
        required: &["url"],
        schema: URL_PROP,
    },
    Tool {
        name: "seo_schema_validate",
        description: "Detect and validate the structured data on a page against schema.org \
                      requirements, listing missing required properties.",
        command: &["schema-validate"],
        args: &[Arg::Flag("url")],
        required: &["url"],
        schema: URL_PROP,
    },
    Tool {
        name: "seo_site_crawl",
        description: "Crawl a whole site and aggregate the issues that only appear across pages: \
                      duplicate titles, broken internal links, thin content, missing schema.",
        command: &["crawl"],
        args: &[
            Arg::Positional("url"),
            Arg::Flag("max-pages"),
            Arg::Flag("max-depth"),
            Arg::Flag("concurrency"),
        ],
        required: &["url"],
        schema: r#"{"url":{"type":"string","description":"Seed URL"},
            "max-pages":{"type":"integer","description":"Page budget (default 100)"},
            "max-depth":{"type":"integer","description":"Link depth from the seed (default 3)"},
            "concurrency":{"type":"integer","description":"Parallel fetches, 1-16 (default 4)"}}"#,
    },
    Tool {
        name: "ai_ask",
        description:
            "Ask the answer engines a question directly and report which brands they name \
                      and which sources they cite. Needs the caller's own API keys.",
        command: &["ask"],
        args: &[
            Arg::Positional("prompt"),
            Arg::Flag("provider"),
            Arg::Flag("brand"),
        ],
        required: &["prompt"],
        schema: r#"{"prompt":{"type":"string","description":"The question to ask"},
            "provider":{"type":"string","description":"Comma-separated: perplexity, openai, anthropic, gemini, ollama. Defaults to every configured provider."},
            "brand":{"type":"string","description":"Brand to check for in the answers"}}"#,
    },
    Tool {
        name: "ai_visibility_run",
        description: "Run a prompt set against the answer engines and measure brand mention rate, \
                      share of voice against named competitors, and which domains get cited. \
                      Stores the run so later runs can be compared.",
        command: &["visibility", "run"],
        args: &[
            Arg::Flag("brand"),
            Arg::Flag("domain"),
            Arg::Flag("prompts"),
            Arg::Repeated("competitor"),
            Arg::Flag("provider"),
            Arg::Flag("label"),
        ],
        required: &["brand"],
        schema: r#"{"brand":{"type":"string"},
            "domain":{"type":"string","description":"The brand's own domain, to detect self-citations"},
            "prompts":{"type":"string","description":"Path to a prompt file (JSON array, {\"prompts\":[...]}, or one per line)"},
            "competitor":{"type":"array","items":{"type":"string"},"description":"Competitor names to score share of voice against"},
            "provider":{"type":"string"},
            "label":{"type":"string","description":"Label for this run, e.g. a release tag"}}"#,
    },
    Tool {
        name: "ai_visibility_history",
        description: "Return the recorded visibility trend for a brand — mention rate, share of \
                      voice, and citation rate over time.",
        command: &["visibility", "history"],
        args: &[Arg::Flag("brand"), Arg::Flag("days")],
        required: &["brand"],
        schema: r#"{"brand":{"type":"string"},"days":{"type":"integer","description":"Look-back window"}}"#,
    },
    Tool {
        name: "ai_crawler_logs",
        description:
            "Parse server access logs and report what the AI crawlers actually did: which \
                      arrived, what they took, what they were refused, and how many human visits \
                      each platform sent back.",
        command: &["logs"],
        args: &[
            Arg::Repeated("file"),
            Arg::Flag("sitemap"),
            Arg::Flag("since"),
        ],
        required: &["file"],
        schema: r#"{"file":{"type":"array","items":{"type":"string"},"description":"Log file paths; .gz is read directly"},
            "sitemap":{"type":"string","description":"Site or sitemap URL, to list pages no AI crawler has fetched"},
            "since":{"type":"string","description":"YYYY-MM-DD lower bound"}}"#,
    },
    Tool {
        name: "seo_content_quality",
        description: "Score content for E-E-A-T signals, thin sections, uncited claims, and \
                      AI-writing patterns.",
        command: &["content-quality"],
        args: &[Arg::Positional("source")],
        required: &["source"],
        schema: r#"{"source":{"type":"string","description":"URL, file path, or - for stdin"}}"#,
    },
    Tool {
        name: "seo_images_audit",
        description: "Audit every image on a page for alt text, explicit dimensions, modern \
                      formats, lazy loading, and LCP impact.",
        command: &["images-audit"],
        args: &[Arg::Flag("url")],
        required: &["url"],
        schema: URL_PROP,
    },
    Tool {
        name: "seo_pagespeed",
        description: "Core Web Vitals for a URL — lab scores from PageSpeed Insights and field \
                      data from CrUX where it exists. Needs GOOGLE_API_KEY.",
        command: &["pagespeed"],
        args: &[Arg::Positional("url"), Arg::Flag("strategy")],
        required: &["url"],
        schema: r#"{"url":{"type":"string"},"strategy":{"type":"string","enum":["mobile","desktop","both"]}}"#,
    },
    Tool {
        name: "seo_drift_compare",
        description: "Compare a page against its stored baseline and report SEO regressions \
                      introduced since the last capture.",
        command: &["drift", "compare"],
        args: &[Arg::Positional("url")],
        required: &["url"],
        schema: URL_PROP,
    },
];

pub fn serve() -> CmdResult<ExitCode> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    // Progress and errors must never touch stdout while the transport owns it.
    eprintln!(
        "seogeo mcp — {} tools, protocol {PROTOCOL_VERSION}",
        TOOLS.len()
    );

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                write_msg(
                    &mut stdout,
                    &error_response(Value::Null, -32700, &format!("parse error: {e}")),
                )?;
                continue;
            }
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

        // Notifications carry no id and must not be answered at all.
        let is_notification = msg.get("id").is_none();

        let response = match method {
            "initialize" => Some(success(
                id,
                json!({
                    "protocolVersion": params
                        .get("protocolVersion")
                        .and_then(Value::as_str)
                        .unwrap_or(PROTOCOL_VERSION),
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "seogeo", "version": env!("CARGO_PKG_VERSION")},
                }),
            )),
            "tools/list" => Some(success(id, json!({"tools": tool_definitions()}))),
            "tools/call" => Some(call_tool(id, &params)),
            "ping" => Some(success(id, json!({}))),
            _ if is_notification => None,
            other => Some(error_response(
                id,
                -32601,
                &format!("unknown method {other}"),
            )),
        };

        if let Some(resp) = response {
            if !is_notification {
                write_msg(&mut stdout, &resp)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn write_msg(out: &mut std::io::Stdout, value: &Value) -> CmdResult {
    let s = serde_json::to_string(value)?;
    writeln!(out, "{s}")?;
    out.flush()?;
    Ok(())
}

fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_definitions() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|t| {
            let properties: Value = serde_json::from_str(t.schema).unwrap_or_else(|_| json!({}));
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": {
                    "type": "object",
                    "properties": properties,
                    "required": t.required,
                },
            })
        })
        .collect()
}

fn call_tool(id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
        return error_response(id, -32602, &format!("unknown tool {name}"));
    };

    for req in tool.required {
        if args.get(*req).is_none() {
            return tool_error(id, &format!("missing required argument {req:?}"));
        }
    }

    let mut argv: Vec<String> = tool.command.iter().map(|s| s.to_string()).collect();
    for arg in tool.args {
        match arg {
            Arg::Positional(key) => {
                if let Some(v) = scalar(&args, key) {
                    argv.push(v);
                }
            }
            Arg::Flag(key) => {
                if let Some(v) = scalar(&args, key) {
                    argv.push(format!("--{key}"));
                    argv.push(v);
                }
            }
            Arg::Repeated(key) => match args.get(*key) {
                Some(Value::Array(items)) => {
                    for item in items {
                        if let Some(s) = as_scalar(item) {
                            argv.push(format!("--{key}"));
                            argv.push(s);
                        }
                    }
                }
                Some(other) => {
                    if let Some(s) = as_scalar(other) {
                        argv.push(format!("--{key}"));
                        argv.push(s);
                    }
                }
                None => {}
            },
        }
    }
    argv.push("--json".to_string());

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return tool_error(id, &format!("cannot locate seogeo binary: {e}")),
    };
    let output = match std::process::Command::new(exe).args(&argv).output() {
        Ok(o) => o,
        Err(e) => return tool_error(id, &format!("failed to run seogeo {}: {e}", argv.join(" "))),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    // A non-zero exit is often a finding ("regressed", "critical"), not a
    // crash, and the JSON body carries the detail. Only report a tool error
    // when there is no body to return.
    if stdout.is_empty() {
        let detail = if stderr.is_empty() {
            format!("seogeo {} produced no output", argv.join(" "))
        } else {
            stderr
        };
        return tool_error(id, &detail);
    }

    success(
        id,
        json!({
            "content": [{"type": "text", "text": stdout}],
            "isError": false,
        }),
    )
}

fn tool_error(id: Value, message: &str) -> Value {
    // Tool-level failures come back as a result with `isError`, not a
    // protocol error, so the model can read the message and adjust.
    success(
        id,
        json!({
            "content": [{"type": "text", "text": message}],
            "isError": true,
        }),
    )
}

fn scalar(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(as_scalar)
}

fn as_scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_valid_schema_json() {
        for t in TOOLS {
            let parsed: Value = serde_json::from_str(t.schema)
                .unwrap_or_else(|e| panic!("{}: bad schema — {e}", t.name));
            assert!(parsed.is_object(), "{}: schema must be an object", t.name);
            for req in t.required {
                assert!(
                    parsed.get(*req).is_some(),
                    "{}: required arg {req:?} is not in the schema",
                    t.name
                );
            }
        }
    }

    #[test]
    fn tool_names_are_unique() {
        let mut names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate tool name");
    }

    #[test]
    fn required_args_are_declared_as_args() {
        for t in TOOLS {
            for req in t.required {
                let declared = t.args.iter().any(|a| {
                    matches!(a,
                        Arg::Positional(k) | Arg::Flag(k) | Arg::Repeated(k)
                        if k == req)
                });
                assert!(declared, "{}: required {req:?} has no Arg mapping", t.name);
            }
        }
    }

    #[test]
    fn unknown_tool_is_a_protocol_error() {
        let r = call_tool(json!(1), &json!({"name": "nope", "arguments": {}}));
        assert!(r["error"].is_object());
    }

    #[test]
    fn missing_required_argument_is_a_tool_error_not_a_crash() {
        let r = call_tool(
            json!(1),
            &json!({"name": "geo_citability", "arguments": {}}),
        );
        assert_eq!(r["result"]["isError"], json!(true));
    }
}
