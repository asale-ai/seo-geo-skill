---
name: seo-schema
description: >
  Detect, validate, and generate Schema.org structured data. JSON-LD format
  preferred. Use when user says "schema", "structured data", "rich results",
  "JSON-LD", or "markup".
user-invocable: true
argument-hint: "[url]"
license: MIT
metadata:
  author: AgriciDaniel
  version: "2.2.4"
  category: seo
---

# Schema Markup Analysis & Generation

## Detection

1. Scan page source for JSON-LD `<script type="application/ld+json">`
2. Check for Microdata (`itemscope`, `itemprop`)
3. Check for RDFa (`typeof`, `property`)
4. Always recommend JSON-LD as primary format (Google's stated preference)

## Validation

- Check required properties per schema type
- Validate against Google's supported rich result types
- Test for common errors:
  - Missing @context
  - Invalid @type
  - Wrong data types
  - Placeholder text
  - Relative URLs (should be absolute)
  - Invalid date formats
- Flag deprecated types (see below)

## Schema Type Status (as of June 2026)

Read `../seo/references/schema-types.md` for the full list. Key rules:

### ACTIVE (recommend freely):
Organization, LocalBusiness, SoftwareApplication, WebApplication, Product (with Certification markup as of April 2025), ProductGroup, Offer, Service, Article, BlogPosting, NewsArticle, Review, AggregateRating, BreadcrumbList, WebSite, WebPage, Person, ProfilePage, ContactPage, VideoObject, ImageObject, Event, JobPosting, Course, DiscussionForumPosting

### VIDEO & SPECIALIZED (recommend freely):
BroadcastEvent, Clip, SeekToAction, SoftwareSourceCode

See `schema/templates.json` for ready-to-use JSON-LD templates for these types.

> **JSON-LD and JavaScript rendering:** Per Google's December 2025 JS SEO guidance, structured data injected via JavaScript may face delayed processing. For time-sensitive markup (especially Product, Offer), include JSON-LD in the initial server-rendered HTML.

### NO RICH RESULTS, KEEP IF USEFUL:
- **FAQPage**: Google retired FAQ rich results for ALL sites on May 7, 2026 (supersedes the Aug 2023 gov/health restriction). No Google SERP rich-result benefit; flag existing FAQPage at Info (not Critical) rather than removal. For genuine user Q&A pages, use **QAPage**.

### DEPRECATED (never recommend):
- **HowTo**: Rich results removed September 2023
- **SpecialAnnouncement**: Deprecated July 31, 2025
- **CourseInfo, EstimatedSalary, LearningVideo**: Retired June 2025
- **ClaimReview**: Retired from rich results June 2025
- **VehicleListing**: Retired from rich results June 2025
- **Practice Problem**: Deprecation notice 2025-11-05; Search Console / Rich Results Test support removed 2026-01-06
- **Book Actions**: Deprecated/removed from Google rich results; do not recommend it for SERP features.
- Search Console / Rich Results Test / appearance-filter support for CourseInfo, EstimatedSalary, LearningVideo, SpecialAnnouncement, VehicleListing was removed 2025-09-09; Practice Problem support was removed 2026-01-06.

### Supported for Dataset Search only:
- **Dataset**: Not discontinued; consumed by Google Dataset Search, with no Google Search rich-result surface. Don't advise removal as if it were killed.

### Still supported (do not flag):
- QAPage (expanded comment-thread properties 2026-03-24), DiscussionForumPosting, Education Q&A (Quiz / `eduQuestionType=Flashcard`). For e-commerce, **hasAdultConsideration** (added 2026-05-22; value `https://schema.org/SexualContentConsideration`) is required for adult products.

## Generation

When generating schema for a page:
1. Identify page type from content analysis
2. Select appropriate schema type(s)
3. Generate valid JSON-LD with all required + recommended properties
4. Include only truthful, verifiable data. Use placeholders clearly marked for user to fill
5. Validate output before presenting

## Common Schema Templates

### Organization
```json
{
  "@context": "https://schema.org",
  "@type": "Organization",
  "name": "[Company Name]",
  "url": "[Website URL]",
  "logo": "[Logo URL]",
  "contactPoint": {
    "@type": "ContactPoint",
    "telephone": "[Phone]",
    "contactType": "customer service"
  },
  "sameAs": [
    "[Facebook URL]",
    "[LinkedIn URL]",
    "[Twitter URL]"
  ]
}
```

### LocalBusiness
```json
{
  "@context": "https://schema.org",
  "@type": "LocalBusiness",
  "name": "[Business Name]",
  "address": {
    "@type": "PostalAddress",
    "streetAddress": "[Street]",
    "addressLocality": "[City]",
    "addressRegion": "[State]",
    "postalCode": "[ZIP]",
    "addressCountry": "US"
  },
  "telephone": "[Phone]",
  "openingHours": "Mo-Fr 09:00-17:00",
  "geo": {
    "@type": "GeoCoordinates",
    "latitude": "[Lat]",
    "longitude": "[Long]"
  }
}
```

### Article/BlogPosting
```json
{
  "@context": "https://schema.org",
  "@type": "Article",
  "headline": "[Title]",
  "author": {
    "@type": "Person",
    "name": "[Author Name]"
  },
  "datePublished": "[YYYY-MM-DD]",
  "dateModified": "[YYYY-MM-DD]",
  "image": "[Image URL]",
  "publisher": {
    "@type": "Organization",
    "name": "[Publisher]",
    "logo": {
      "@type": "ImageObject",
      "url": "[Logo URL]"
    }
  }
}
```

## Output

- `SCHEMA-REPORT.md`: detection and validation results
- `generated-schema.json`: ready-to-use JSON-LD snippets

### Validation Results
| Schema | Type | Status | Issues |
|--------|------|--------|--------|
| ... | ... | ✅/⚠️/❌ | ... |

### Recommendations
- Missing schema opportunities
- Validation fixes needed
- Generated code for implementation

## Error Handling

| Scenario | Action |
|----------|--------|
| URL unreachable | Report connection error with status code. Suggest verifying URL and checking if the page requires authentication. |
| No schema markup found | Report that no JSON-LD, Microdata, or RDFa was detected. Recommend appropriate schema types based on page content analysis. |
| Invalid JSON-LD syntax | Parse and report specific syntax errors (missing brackets, trailing commas, unquoted keys). Provide corrected JSON-LD output. |
| Deprecated schema type detected | Flag the deprecated type with its retirement date. Recommend the current replacement type or advise removal if no replacement exists. |

---

## Tooling

```bash
# Detect and validate every JSON-LD block on a page
seogeo schema-validate https://example.com --json
seogeo schema-validate --file page.html --json

# Generate JSON-LD for the high-leverage types
seogeo schema-generate organization --name "Example Corp" \
  --url https://example.com --logo https://example.com/logo.png \
  --same-as https://linkedin.com/company/example https://github.com/example

seogeo schema-generate local-business --name "Example Cafe" \
  --url https://example.com --business-type CafeOrCoffeeShop \
  --street "1 Main St" --city Austin --region TX --postal-code 78701 \
  --country US --telephone "+1-512-555-0100" \
  --hours "Mo-Fr 07:00-18:00" --hours "Sa-Su 08:00-16:00"

seogeo schema-generate profile --name "Jane Rivera" \
  --url https://example.com/about --job-title "Head of Research" \
  --same-as https://orcid.org/0000-0000-0000-0000 \
  --knows-about "Core Web Vitals" "Structured data"

seogeo schema-generate discussion --headline "How do you score INP?" \
  --author "Sara Park" --url https://forum.example.com/t/123 \
  --date 2026-05-12T14:00:00Z --likes 42

seogeo schema-generate reservation --provider "Marea NYC" \
  --start 2026-06-04T19:30:00-04:00 --party-size 4

seogeo schema-generate order --merchant "Acme Pizza" \
  --order-url https://acme.example/order

# Paste-ready output
seogeo schema-generate organization --name X --url https://x.test --script-tag
```

`schema-validate` separates `errors` (the rich result will not show) from
`warnings` (it will show, but thinner). It also reports
`retired_for_google_rich_results` — FAQPage, HowTo, and Dataset no longer
produce Google Search rich results. Do not tell a user to remove them:
they still carry meaning for other consumers. Tell them not to expect a
rich result.

`--same-as` and `--knows-about` on `profile` and `organization` are the
cheapest entity-graph work available. Link to Wikipedia, Wikidata, ORCID,
GitHub, and LinkedIn so the entity resolves to one identity across
knowledge graphs.
