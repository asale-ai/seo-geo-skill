//! Command-line surface.
//!
//! Every variant here corresponds to exactly one invocation documented in a
//! `SKILL.md`. `seogeo commands` prints the mapping so the correspondence
//! can be checked mechanically (see `tests/skill_cli_parity.rs`).

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "seogeo",
    version,
    about = "Execution engine for the SEO + GEO agent skills",
    long_about = "seogeo runs every analysis the bundled SEO and GEO skills need: page \
                  fetching, HTML/SEO parsing, AI-citability scoring, crawler and llms.txt \
                  audits, drift tracking, Google/Moz/Bing/DataForSEO API access, and \
                  report generation — from a single static binary."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    Auto,
    Always,
    Never,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormFactor {
    Phone,
    Desktop,
    Tablet,
    All,
}

impl FormFactor {
    pub fn as_api(self) -> &'static str {
        match self {
            FormFactor::Phone => "PHONE",
            FormFactor::Desktop => "DESKTOP",
            FormFactor::Tablet => "TABLET",
            FormFactor::All => "ALL_FORM_FACTORS",
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    Mobile,
    Desktop,
    Both,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallTarget {
    /// Claude Code — ~/.claude/skills/<name>/SKILL.md
    Claude,
    /// OpenAI Codex CLI — ~/.codex/skills/<name>/SKILL.md
    Codex,
    /// Gemini CLI — ~/.gemini/extensions/<name>/ with gemini-extension.json
    Gemini,
    /// OpenCode — ~/.config/opencode/skill/<name>/SKILL.md
    Opencode,
    /// Generic AGENTS.md-based agents — ~/.agents/skills/<name>/
    Agents,
    /// Every target detected on this machine
    All,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    // ---------------------------------------------------------------- core
    /// Validate a URL against the SSRF policy (parse-time or DNS-strict)
    UrlSafety {
        url: String,
        /// Resolve DNS and refuse any non-public record
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        json: bool,
    },

    /// Fetch a page with SSRF-safe HTTP, optionally through a headless render
    Fetch {
        url: String,
        /// Write the body here instead of stdout
        #[arg(short, long)]
        output: Option<String>,
        #[arg(short, long, default_value_t = 30)]
        timeout: u64,
        #[arg(long)]
        no_redirects: bool,
        #[arg(long)]
        user_agent: Option<String>,
        /// Fetch as Googlebot to detect prerender / dynamic rendering
        #[arg(long)]
        googlebot: bool,
        #[arg(long, value_enum, default_value = "never")]
        render: RenderMode,
        /// Emit the full response record as JSON instead of raw HTML
        #[arg(long)]
        json: bool,
    },

    /// Extract SEO elements from HTML (file, stdin, or a live URL)
    Parse {
        /// HTML file to parse; omit to read stdin, or pass --url to fetch
        file: Option<String>,
        /// Base URL for link resolution, or the URL to fetch when no file is given
        #[arg(short, long)]
        url: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Render a page in headless Chrome (SPA-aware)
    Render {
        url: String,
        #[arg(long, value_enum, default_value = "auto")]
        mode: RenderMode,
        #[arg(long, default_value_t = 30000)]
        timeout_ms: u64,
        /// Also dump the accessibility tree
        #[arg(long)]
        a11y_tree: bool,
        #[arg(long)]
        user_agent: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Discover sitemaps via robots.txt and the common well-known paths
    SitemapDiscovery {
        url: String,
        #[arg(long)]
        json: bool,
    },

    /// Audit robots.txt for AI-crawler access
    Robots {
        url: String,
        #[arg(long)]
        json: bool,
    },

    /// Validate or generate llms.txt
    LlmsTxt {
        #[command(subcommand)]
        action: LlmsTxtAction,
    },

    /// Split a page into heading-delimited content blocks
    Blocks {
        url: String,
        #[arg(long, default_value_t = 20)]
        min_words: usize,
        #[arg(long)]
        json: bool,
    },

    /// List URLs from a site's XML sitemaps
    CrawlSitemap {
        url: String,
        #[arg(long, default_value_t = 50)]
        max_pages: usize,
        #[arg(long)]
        json: bool,
    },

    // ----------------------------------------------------------------- geo
    /// Score a page's passages for AI citation readiness
    Citability {
        /// URL to analyse
        url: Option<String>,
        /// Local HTML file instead of a URL
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Scan brand presence across the platforms AI answers cite
    BrandScan {
        brand: String,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        json: bool,
    },

    // ------------------------------------------------------------- content
    /// Score text against the QRG content-quality heuristics
    ContentQuality {
        #[arg(default_value = "-")]
        source: String,
        #[arg(long, default_value_t = 60)]
        threshold: i64,
        #[arg(long)]
        json: bool,
    },

    /// Rewrite AI-typical phrasing into direct prose
    ContentHumanize {
        #[arg(default_value = "-")]
        source: String,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Extract verifiable claims and flag the ones without a citation
    ContentVerify {
        #[arg(default_value = "-")]
        source: String,
        #[arg(long, default_value_t = 0.4)]
        threshold: f64,
        #[arg(long)]
        json: bool,
    },

    /// Entity + sentiment analysis via the Cloud Natural Language API
    NlpAnalyze {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        file: Option<String>,
        /// entities, sentiment, categories, syntax (repeatable)
        #[arg(long, value_delimiter = ',')]
        features: Vec<String>,
        #[arg(long)]
        json: bool,
    },

    // -------------------------------------------------------------- schema
    /// Generate JSON-LD for the high-leverage Schema.org types
    SchemaGenerate {
        #[command(subcommand)]
        kind: SchemaKind,
        #[arg(long, default_value_t = 2, global = true)]
        indent: usize,
        /// Wrap output in a <script type="application/ld+json"> tag
        #[arg(long, global = true)]
        script_tag: bool,
    },

    /// Detect and validate all structured data on a page
    SchemaValidate {
        url: Option<String>,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Validate Product schema against merchant-listing requirements
    SchemaEcommerce {
        url: Option<String>,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        json: bool,
    },

    // --------------------------------------------------------------- drift
    /// SEO drift monitoring: baseline, compare, history, report
    Drift {
        #[command(subcommand)]
        action: DriftAction,
    },

    // ----------------------------------------------------------- technical
    /// Audit Speculation Rules, bfcache, prerender, and LCP preload signals
    PreloadCheck {
        url: String,
        #[arg(long)]
        json: bool,
    },

    /// Decompose LCP into its four CrUX subparts
    LcpSubparts {
        url: String,
        #[arg(long, value_enum, default_value = "phone")]
        form_factor: FormFactor,
        #[arg(long)]
        json: bool,
    },

    /// Parasite-SEO / site-reputation-abuse risk by subfolder
    ParasiteRisk {
        urls: Vec<String>,
        #[arg(long)]
        urls_file: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Audit a store's Universal Commerce Protocol profile
    UcpCheck {
        site: String,
        #[arg(long)]
        probe_endpoints: bool,
        #[arg(long, default_value_t = 10)]
        timeout: u64,
        #[arg(long)]
        json: bool,
    },

    /// Lint for retired Google Business Profile features
    GbpLint {
        source: String,
        /// Treat SOURCE as a local file rather than a URL
        #[arg(long)]
        file: bool,
        #[arg(long)]
        json: bool,
    },

    /// WHOIS heritage check for expired-domain abuse patterns
    DomainHistory {
        domain: String,
        #[arg(long)]
        topic: Option<String>,
        #[arg(long)]
        baseline_topic: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Audit a page for agent/LLM readability (no-JS content, semantics)
    AgentUx {
        url: String,
        #[arg(long)]
        json: bool,
    },

    /// Validate hreflang / international SEO annotations
    Hreflang {
        url: Option<String>,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Image optimisation audit: alt text, dimensions, formats, lazy loading
    ImagesAudit {
        url: Option<String>,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// IPTC DigitalSourceType audit / injection for AI-generated imagery
    Iptc {
        #[command(subcommand)]
        action: IptcAction,
    },

    // -------------------------------------------------------- google APIs
    /// Inspect and configure Google API credentials
    GoogleAuth {
        /// Check credentials, optionally for one service
        #[arg(long, num_args = 0..=1, default_missing_value = "all")]
        check: Option<String>,
        /// Print setup instructions
        #[arg(long)]
        setup: bool,
        /// Print the detected capability tier
        #[arg(long)]
        tier: bool,
        #[arg(long)]
        json: bool,
    },

    /// PageSpeed Insights v5 + CrUX field data
    Pagespeed {
        url: String,
        #[arg(long, value_enum, default_value = "mobile")]
        strategy: Strategy,
        #[arg(long)]
        psi_only: bool,
        #[arg(long)]
        crux_only: bool,
        #[arg(long)]
        json: bool,
    },

    /// CrUX History API — 25 weeks of field data
    CruxHistory {
        url: String,
        #[arg(long, value_enum, default_value = "phone")]
        form_factor: FormFactor,
        /// Query the origin rather than the exact URL
        #[arg(long)]
        origin: bool,
        #[arg(long)]
        json: bool,
    },

    /// Search Console Search Analytics
    GscQuery {
        #[arg(long)]
        property: Option<String>,
        #[arg(long, default_value = "query")]
        dimensions: String,
        #[arg(short, long, default_value_t = 28)]
        days: i64,
        #[arg(long)]
        start_date: Option<String>,
        #[arg(long)]
        end_date: Option<String>,
        #[arg(long, default_value = "web")]
        r#type: String,
        #[arg(long, default_value_t = 1000)]
        limit: u32,
        #[arg(long)]
        country: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Search Console submitted sitemaps
    GscSitemaps {
        #[arg(long)]
        property: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Search Console verified properties
    GscSites {
        #[arg(long)]
        json: bool,
    },

    /// Search Console URL Inspection (single or batch)
    GscInspect {
        url: Option<String>,
        #[arg(short, long)]
        batch: Option<String>,
        #[arg(long)]
        property: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Indexing API v3 notifications
    IndexingNotify {
        url: Option<String>,
        #[arg(short, long)]
        batch: Option<String>,
        #[arg(long, default_value = "URL_UPDATED")]
        r#type: String,
        /// Read notification metadata for a URL instead of publishing
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// GA4 organic traffic reports
    Ga4Report {
        #[arg(long)]
        property: Option<String>,
        /// organic, top-pages, devices, countries
        #[arg(long, default_value = "organic")]
        report: String,
        #[arg(short, long, default_value_t = 28)]
        days: i64,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },

    /// Google Ads Keyword Planner
    KeywordPlanner {
        #[command(subcommand)]
        action: KeywordAction,
    },

    /// YouTube Data API v3
    YoutubeSearch {
        #[command(subcommand)]
        action: YoutubeAction,
    },

    /// Render a client-ready HTML/PDF report from audit JSON
    GoogleReport {
        #[arg(long)]
        r#type: String,
        #[arg(long)]
        data: String,
        #[arg(long)]
        domain: String,
        /// html or pdf (pdf needs headless Chrome)
        #[arg(long, default_value = "html")]
        format: String,
        #[arg(long)]
        output_dir: Option<String>,
    },

    // ----------------------------------------------------------- backlinks
    /// Inspect backlink API credentials (Moz, Bing)
    BacklinksAuth {
        #[arg(long, num_args = 0..=1, default_missing_value = "all")]
        check: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Moz Link Explorer
    Moz {
        #[command(subcommand)]
        action: MozAction,
    },

    /// Bing Webmaster Tools
    Bing {
        #[command(subcommand)]
        action: BingAction,
    },

    /// Common Crawl web-graph rank and in-degree
    Commoncrawl {
        domain: String,
        /// How much of the (sorted, gzipped) rank file to scan, in MiB.
        /// The file is ordered by rank, so well-known domains appear early.
        #[arg(long, default_value_t = 64)]
        max_scan_mb: usize,
        #[arg(long)]
        json: bool,
    },

    /// Verify that claimed backlinks actually exist and are followed
    VerifyBacklinks {
        #[arg(long)]
        target: String,
        /// File of candidate linking URLs, one per line
        #[arg(long)]
        links: String,
        #[arg(long, default_value_t = 20)]
        timeout: u64,
        #[arg(long)]
        json: bool,
    },

    /// Validate a backlink report file for structural and evidence gaps
    ValidateBacklinkReport {
        file: String,
        #[arg(long)]
        json: bool,
    },

    // --------------------------------------------------------- dataforseo
    /// DataForSEO cost estimation and budget tracking
    DataforseoCosts {
        #[command(subcommand)]
        action: DfsCostAction,
    },

    /// Normalise a DataForSEO response into the shape the skills expect
    DataforseoNormalize {
        file: String,
        #[arg(long)]
        module: String,
        #[arg(long)]
        json: bool,
    },

    /// DataForSEO Merchant API (Google Shopping / Amazon)
    DataforseoMerchant {
        #[command(subcommand)]
        action: DfsMerchantAction,
    },

    // --------------------------------------------------------------- misc
    /// Submit URLs to IndexNow (Bing, Yandex, Seznam, Naver)
    Indexnow {
        #[arg(long)]
        host: String,
        #[arg(long, num_args = 1..)]
        urls: Vec<String>,
        #[arg(long)]
        urls_file: Option<String>,
        /// Only check that the key file is reachable
        #[arg(long)]
        verify_only: bool,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Query the bundled Google Search ranking-update timeline
    SeoUpdates {
        /// Only show updates on or after this date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Sync the FLOW prompt library from its upstream repository
    SyncFlow {
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = "main")]
        r#ref: String,
        #[arg(long)]
        json: bool,
    },

    /// Run a site-wide Lighthouse sweep through the Unlighthouse CLI
    Unlighthouse {
        url: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },

    /// Capture a screenshot with headless Chrome
    Screenshot {
        url: String,
        #[arg(short, long, default_value = "screenshot.png")]
        output: String,
        #[arg(long, default_value = "1280x800")]
        viewport: String,
        #[arg(long)]
        full_page: bool,
        #[arg(long)]
        json: bool,
    },

    /// CRM-lite prospect and client pipeline
    Crm {
        #[command(subcommand)]
        action: CrmAction,
    },

    /// Convert an audit markdown report into a styled HTML/PDF deliverable
    ReportPdf {
        /// Markdown report to convert
        input: String,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(long)]
        brand: Option<String>,
        #[arg(long)]
        score: Option<String>,
        /// Emit HTML only and skip the Chrome print step
        #[arg(long)]
        html_only: bool,
    },

    /// Install the bundled skills into one or more agent tools
    Install {
        #[arg(long, value_enum, default_value = "claude")]
        target: InstallTarget,
        /// Override the install root for the chosen target
        #[arg(long)]
        dir: Option<String>,
        /// Install only these skills (default: all)
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Show what would be written without writing it
        #[arg(long)]
        dry_run: bool,
        /// List detected targets and their install paths
        #[arg(long)]
        list: bool,
        #[arg(long)]
        json: bool,
    },

    /// Print the subcommand ↔ SKILL.md invocation map
    Commands {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum LlmsTxtAction {
    /// Fetch /llms.txt and validate it against the spec
    Validate {
        url: String,
        #[arg(long)]
        json: bool,
    },
    /// Crawl the site and generate llms.txt + llms-full.txt
    Generate {
        url: String,
        #[arg(long, default_value_t = 30)]
        max_pages: usize,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SchemaKind {
    /// Reservation (FoodEstablishmentReservation and friends)
    Reservation(ReservationArgs),
    /// OrderAction potentialAction
    Order(OrderArgs),
    /// DiscussionForumPosting
    Discussion(DiscussionArgs),
    /// ProfilePage with sameAs / knowsAbout
    Profile(ProfileArgs),
    /// Organization with sameAs entity links
    Organization(OrganizationArgs),
    /// LocalBusiness with address, hours, and geo
    LocalBusiness(LocalBusinessArgs),
}

#[derive(Args, Debug)]
pub struct ReservationArgs {
    #[arg(long)]
    pub provider: String,
    /// ISO 8601 startTime
    #[arg(long)]
    pub start: String,
    #[arg(long)]
    pub end: Option<String>,
    #[arg(long)]
    pub party_size: Option<u32>,
    #[arg(long)]
    pub reservation_id: Option<String>,
    #[arg(long)]
    pub reservation_for_name: Option<String>,
    #[arg(long)]
    pub customer_name: Option<String>,
    #[arg(long)]
    pub customer_email: Option<String>,
    #[arg(long, default_value = "FoodEstablishmentReservation")]
    pub reservation_kind: String,
}

#[derive(Args, Debug)]
pub struct OrderArgs {
    #[arg(long)]
    pub merchant: String,
    #[arg(long)]
    pub order_url: String,
    #[arg(long, default_value = "Order online")]
    pub name: String,
    #[arg(long, num_args = 0..)]
    pub accepted_payment_method: Vec<String>,
    #[arg(long, num_args = 0..)]
    pub delivery_method: Vec<String>,
}

#[derive(Args, Debug)]
pub struct DiscussionArgs {
    #[arg(long)]
    pub headline: String,
    #[arg(long)]
    pub author: String,
    #[arg(long)]
    pub url: String,
    #[arg(long)]
    pub date: String,
    #[arg(long)]
    pub text: Option<String>,
    #[arg(long)]
    pub date_modified: Option<String>,
    #[arg(long)]
    pub comment_count: Option<u32>,
    #[arg(long)]
    pub likes: Option<u32>,
}

#[derive(Args, Debug)]
pub struct ProfileArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub url: String,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long, num_args = 0..)]
    pub same_as: Vec<String>,
    #[arg(long, num_args = 0..)]
    pub knows_about: Vec<String>,
    #[arg(long)]
    pub works_for: Option<String>,
    #[arg(long)]
    pub image: Option<String>,
    #[arg(long)]
    pub job_title: Option<String>,
}

#[derive(Args, Debug)]
pub struct OrganizationArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub url: String,
    #[arg(long)]
    pub logo: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long, num_args = 0..)]
    pub same_as: Vec<String>,
    #[arg(long)]
    pub telephone: Option<String>,
    #[arg(long)]
    pub email: Option<String>,
}

#[derive(Args, Debug)]
// Longitudes are routinely negative; without this clap reads `-97.7` as a flag.
#[command(allow_negative_numbers = true)]
pub struct LocalBusinessArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub url: String,
    #[arg(long, default_value = "LocalBusiness")]
    pub business_type: String,
    #[arg(long)]
    pub street: Option<String>,
    #[arg(long)]
    pub city: Option<String>,
    #[arg(long)]
    pub region: Option<String>,
    #[arg(long)]
    pub postal_code: Option<String>,
    #[arg(long)]
    pub country: Option<String>,
    #[arg(long)]
    pub telephone: Option<String>,
    #[arg(long)]
    pub latitude: Option<f64>,
    #[arg(long)]
    pub longitude: Option<f64>,
    /// e.g. "Mo-Fr 09:00-17:00" (repeatable)
    #[arg(long, num_args = 0..)]
    pub hours: Vec<String>,
    #[arg(long, num_args = 0..)]
    pub same_as: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum DriftAction {
    /// Capture a known-good snapshot of a page's SEO-critical elements
    Baseline {
        url: String,
        #[arg(long)]
        skip_cwv: bool,
        #[arg(long)]
        json: bool,
    },
    /// Compare the live page against its baseline across 17 rules
    Compare {
        url: String,
        #[arg(long)]
        skip_cwv: bool,
        #[arg(long)]
        baseline_id: Option<i64>,
        #[arg(long)]
        json: bool,
    },
    /// List stored baselines and comparisons for a URL
    History {
        url: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Render a comparison JSON file as a standalone HTML report
    Report {
        input: String,
        #[arg(short, long, default_value = "drift-report.html")]
        output: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum IptcAction {
    /// Report DigitalSourceType coverage for an image or directory
    Audit {
        path: String,
        #[arg(long)]
        json: bool,
    },
    /// Write an IPTC DigitalSourceType sidecar for an AI-generated image
    Inject {
        path: String,
        #[arg(long, default_value = "trainedAlgorithmicMedia")]
        source_type: String,
        #[arg(long)]
        creator: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum KeywordAction {
    /// Keyword ideas for seed terms or a URL
    Ideas {
        seeds: Vec<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value = "US")]
        country: String,
        #[arg(long)]
        json: bool,
    },
    /// Historical search volume for exact keywords
    Volume {
        keywords: Vec<String>,
        #[arg(long, default_value = "US")]
        country: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum YoutubeAction {
    /// Search videos for a query
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Fetch statistics and metadata for one video
    Video {
        video_id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum MozAction {
    /// Domain Authority, Page Authority, spam score
    Metrics {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Referring domains
    Domains {
        target: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Anchor-text distribution
    Anchors {
        target: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Top linked pages on a domain
    Pages {
        target: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum BingAction {
    /// Inbound links for a registered site
    Links {
        url: String,
        #[arg(long)]
        json: bool,
    },
    /// Link profile comparison between two registered sites
    Compare {
        url_a: String,
        url_b: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum DfsCostAction {
    /// Estimated cost for an endpoint
    Check {
        endpoint: String,
        #[arg(long, default_value_t = 1)]
        count: u32,
        #[arg(long)]
        json: bool,
    },
    /// Record an actual charge against the running budget
    Log {
        endpoint: String,
        cost: f64,
        #[arg(long)]
        json: bool,
    },
    /// Spend to date and remaining budget
    Summary {
        #[arg(long)]
        json: bool,
    },
    /// Every endpoint in the price table
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show or set the spend budget the other commands report against
    Budget {
        /// Set the budget in USD; omit to just show the current one
        #[arg(long)]
        set: Option<f64>,
        /// Remove the budget entirely
        #[arg(long)]
        clear: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum DfsMerchantAction {
    /// Product search on Google Shopping or Amazon
    Search {
        keyword: String,
        #[arg(long, default_value = "google")]
        marketplace: String,
        #[arg(long, default_value = "United States")]
        location: String,
        #[arg(long)]
        json: bool,
    },
    /// Compare a product across marketplaces
    Compare {
        keyword: String,
        #[arg(long, default_value = "United States")]
        location: String,
        #[arg(long)]
        json: bool,
    },
    /// Sellers for a product id
    Sellers {
        product_id: String,
        #[arg(long, default_value = "google")]
        marketplace: String,
        #[arg(long, default_value = "United States")]
        location: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CrmAction {
    /// Add a prospect
    Add {
        domain: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long, default_value = "lead")]
        stage: String,
        #[arg(long)]
        value: Option<f64>,
        #[arg(long)]
        json: bool,
    },
    /// List prospects, optionally filtered by stage
    List {
        #[arg(long)]
        stage: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show one prospect with its full history
    Show {
        domain: String,
        #[arg(long)]
        json: bool,
    },
    /// Move a prospect to another stage or change its deal value
    Update {
        domain: String,
        #[arg(long)]
        stage: Option<String>,
        #[arg(long)]
        value: Option<f64>,
        #[arg(long)]
        json: bool,
    },
    /// Append a dated note
    Note {
        domain: String,
        text: String,
        #[arg(long)]
        json: bool,
    },
    /// Attach an audit score to a prospect
    Audit {
        domain: String,
        #[arg(long)]
        score: f64,
        #[arg(long)]
        report: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Pipeline totals by stage
    Pipeline {
        #[arg(long)]
        json: bool,
    },
    /// Month-over-month delta between two audits for a domain
    Compare {
        domain: String,
        #[arg(long)]
        json: bool,
    },
}
