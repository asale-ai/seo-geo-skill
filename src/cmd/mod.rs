//! Subcommand dispatch.

pub mod backlinks;
pub mod checks;
pub mod content;
pub mod core;
pub mod crm;
pub mod dataforseo;
pub mod drift;
pub mod geo;
pub mod google;
pub mod install;
pub mod misc;
pub mod schema;

use std::process::ExitCode;

use crate::cli::{Cli, Command};
use crate::output::CmdResult;

pub fn dispatch(cli: Cli) -> CmdResult<ExitCode> {
    use Command::*;
    match cli.command {
        // core
        UrlSafety { url, strict, json } => core::url_safety(&url, strict, json),
        Fetch {
            url,
            output,
            timeout,
            no_redirects,
            user_agent,
            googlebot,
            render,
            json,
        } => core::fetch(
            &url,
            output.as_deref(),
            timeout,
            !no_redirects,
            user_agent.as_deref(),
            googlebot,
            render,
            json,
        ),
        Parse { file, url, json } => core::parse(file.as_deref(), url.as_deref(), json),
        Render {
            url,
            mode,
            timeout_ms,
            a11y_tree,
            user_agent,
            json,
        } => core::render(
            &url,
            mode,
            timeout_ms,
            a11y_tree,
            user_agent.as_deref(),
            json,
        ),
        SitemapDiscovery { url, json } => core::sitemap_discovery(&url, json),
        Robots { url, json } => core::robots(&url, json),
        LlmsTxt { action } => core::llms_txt(action),
        Blocks {
            url,
            min_words,
            json,
        } => core::blocks(&url, min_words, json),
        CrawlSitemap {
            url,
            max_pages,
            json,
        } => core::crawl_sitemap(&url, max_pages, json),

        // geo
        Citability { url, file, json } => geo::citability(url.as_deref(), file.as_deref(), json),
        BrandScan {
            brand,
            domain,
            json,
        } => geo::brand_scan(&brand, domain.as_deref(), json),

        // content
        ContentQuality {
            source,
            threshold,
            json,
        } => content::quality(&source, threshold, json),
        ContentHumanize {
            source,
            output,
            json,
        } => content::humanize(&source, output.as_deref(), json),
        ContentVerify {
            source,
            threshold,
            json,
        } => content::verify(&source, threshold, json),
        NlpAnalyze {
            url,
            text,
            file,
            features,
            json,
        } => google::nlp_analyze(
            url.as_deref(),
            text.as_deref(),
            file.as_deref(),
            &features,
            json,
        ),

        // schema
        SchemaGenerate {
            kind,
            indent,
            script_tag,
        } => schema::generate(kind, indent, script_tag),
        SchemaValidate { url, file, json } => {
            schema::validate(url.as_deref(), file.as_deref(), json)
        }
        SchemaEcommerce { url, file, json } => {
            schema::ecommerce(url.as_deref(), file.as_deref(), json)
        }

        // drift
        Drift { action } => drift::run(action),

        // technical checks
        PreloadCheck { url, json } => checks::preload(&url, json),
        LcpSubparts {
            url,
            form_factor,
            json,
        } => google::lcp_subparts(&url, form_factor, json),
        ParasiteRisk {
            urls,
            urls_file,
            json,
        } => checks::parasite_risk(&urls, urls_file.as_deref(), json),
        UcpCheck {
            site,
            probe_endpoints,
            timeout,
            json,
        } => checks::ucp(&site, probe_endpoints, timeout, json),
        GbpLint { source, file, json } => checks::gbp_lint(&source, file, json),
        DomainHistory {
            domain,
            topic,
            baseline_topic,
            json,
        } => checks::domain_history(&domain, topic.as_deref(), baseline_topic.as_deref(), json),
        AgentUx { url, json } => checks::agent_ux(&url, json),
        Hreflang { url, file, json } => checks::hreflang(url.as_deref(), file.as_deref(), json),
        ImagesAudit { url, file, json } => {
            checks::images_audit(url.as_deref(), file.as_deref(), json)
        }
        Iptc { action } => checks::iptc(action),

        // google APIs
        GoogleAuth {
            check,
            setup,
            tier,
            json,
        } => google::auth(check.as_deref(), setup, tier, json),
        Pagespeed {
            url,
            strategy,
            psi_only,
            crux_only,
            json,
        } => google::pagespeed(&url, strategy, psi_only, crux_only, json),
        CruxHistory {
            url,
            form_factor,
            origin,
            json,
        } => google::crux_history(&url, form_factor, origin, json),
        GscQuery {
            property,
            dimensions,
            days,
            start_date,
            end_date,
            r#type,
            limit,
            country,
            json,
        } => google::gsc_query(
            property.as_deref(),
            &dimensions,
            days,
            start_date.as_deref(),
            end_date.as_deref(),
            &r#type,
            limit,
            country.as_deref(),
            json,
        ),
        GscSitemaps { property, json } => google::gsc_sitemaps(property.as_deref(), json),
        GscSites { json } => google::gsc_sites(json),
        GscInspect {
            url,
            batch,
            property,
            json,
        } => google::gsc_inspect(url.as_deref(), batch.as_deref(), property.as_deref(), json),
        IndexingNotify {
            url,
            batch,
            r#type,
            status,
            json,
        } => google::indexing_notify(
            url.as_deref(),
            batch.as_deref(),
            &r#type,
            status.as_deref(),
            json,
        ),
        Ga4Report {
            property,
            report,
            days,
            limit,
            json,
        } => google::ga4_report(property.as_deref(), &report, days, limit, json),
        KeywordPlanner { action } => google::keyword_planner(action),
        YoutubeSearch { action } => google::youtube(action),
        GoogleReport {
            r#type,
            data,
            domain,
            format,
            output_dir,
        } => google::report(&r#type, &data, &domain, &format, output_dir.as_deref()),

        // backlinks
        BacklinksAuth { check, json } => backlinks::auth(check.as_deref(), json),
        Moz { action } => backlinks::moz(action),
        Bing { action } => backlinks::bing(action),
        Commoncrawl {
            domain,
            max_scan_mb,
            json,
        } => backlinks::commoncrawl(&domain, max_scan_mb, json),
        VerifyBacklinks {
            target,
            links,
            timeout,
            json,
        } => backlinks::verify(&target, &links, timeout, json),
        ValidateBacklinkReport { file, json } => backlinks::validate_report(&file, json),

        // dataforseo
        DataforseoCosts { action } => dataforseo::costs(action),
        DataforseoNormalize { file, module, json } => dataforseo::normalize(&file, &module, json),
        DataforseoMerchant { action } => dataforseo::merchant(action),

        // misc
        Indexnow {
            host,
            urls,
            urls_file,
            verify_only,
            key,
            json,
        } => misc::indexnow(
            &host,
            &urls,
            urls_file.as_deref(),
            verify_only,
            key.as_deref(),
            json,
        ),
        SeoUpdates { since, json } => misc::seo_updates(since.as_deref(), json),
        SyncFlow {
            dry_run,
            r#ref,
            json,
        } => misc::sync_flow(dry_run, &r#ref, json),
        Unlighthouse { url, limit, json } => misc::unlighthouse(&url, limit, json),
        Screenshot {
            url,
            output,
            viewport,
            full_page,
            json,
        } => misc::screenshot(&url, &output, &viewport, full_page, json),
        Crm { action } => crm::run(action),
        ReportPdf {
            input,
            output,
            brand,
            score,
            html_only,
        } => misc::report_pdf(
            &input,
            output.as_deref(),
            brand.as_deref(),
            score.as_deref(),
            html_only,
        ),
        Install {
            target,
            dir,
            only,
            dry_run,
            list,
            json,
        } => install::run(target, dir.as_deref(), &only, dry_run, list, json),
        Commands { json } => misc::commands(json),
    }
}
