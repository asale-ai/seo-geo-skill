//! Structured-data generation and validation.
//!
//! Generators emit JSON-LD with `@context: https://schema.org` and absolute
//! URLs — the conventions Google's Rich Results Test enforces. The validator
//! checks required and recommended properties per type rather than shelling
//! out to a remote service, so audits work offline and without quota.

use std::process::ExitCode;

use serde_json::{json, Value};

use crate::cli::SchemaKind;
use crate::cmd::core::fetch_record;
use crate::html;
use crate::output::{err, print_json, CmdResult};

const OK: CmdResult<ExitCode> = Ok(ExitCode::SUCCESS);

// --------------------------------------------------------------- generate

/// Drop null members recursively so generated JSON-LD stays tight.
fn strip_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(strip_nulls).collect()),
        other => other,
    }
}

pub fn generate(kind: SchemaKind, indent: usize, script_tag: bool) -> CmdResult<ExitCode> {
    let payload = match kind {
        SchemaKind::Reservation(a) => {
            let mut p = json!({
                "@context": "https://schema.org",
                "@type": a.reservation_kind,
                "reservationStatus": "https://schema.org/ReservationConfirmed",
                "provider": {"@type": "Organization", "name": a.provider},
                "reservationFor": {
                    "@type": if a.reservation_kind == "FoodEstablishmentReservation" {
                        "FoodEstablishment"
                    } else { "Place" },
                    "name": a.reservation_for_name.clone().unwrap_or_else(|| a.provider.clone()),
                },
                "startTime": a.start,
                "endTime": a.end,
                "partySize": a.party_size,
                "reservationId": a.reservation_id,
            });
            if a.customer_name.is_some() || a.customer_email.is_some() {
                p["underName"] = json!({
                    "@type": "Person",
                    "name": a.customer_name,
                    "email": a.customer_email,
                });
            }
            p
        }
        SchemaKind::Order(a) => {
            let mut p = json!({
                "@context": "https://schema.org",
                "@type": "OrderAction",
                "name": a.name,
                "target": {
                    "@type": "EntryPoint",
                    "urlTemplate": a.order_url,
                    "inLanguage": "en-US",
                    "actionPlatform": [
                        "https://schema.org/DesktopWebPlatform",
                        "https://schema.org/MobileWebPlatform",
                    ],
                },
                "deliveryMethod": if a.delivery_method.is_empty() {
                    json!([
                        "https://schema.org/OnSitePickup",
                        "https://schema.org/ParcelService",
                    ])
                } else { json!(a.delivery_method) },
                "priceSpecification": {
                    "@type": "PriceSpecification",
                    "eligibleTransactionVolume": {
                        "@type": "PriceSpecification",
                        "minPrice": 0,
                        "priceCurrency": "USD",
                    },
                },
                "merchant": {"@type": "Organization", "name": a.merchant},
            });
            if !a.accepted_payment_method.is_empty() {
                p["acceptedPaymentMethod"] = json!(a
                    .accepted_payment_method
                    .iter()
                    .map(|m| json!({"@type": "PaymentMethod", "name": m}))
                    .collect::<Vec<_>>());
            }
            p
        }
        SchemaKind::Discussion(a) => {
            let mut p = json!({
                "@context": "https://schema.org",
                "@type": "DiscussionForumPosting",
                "headline": a.headline,
                "author": {"@type": "Person", "name": a.author},
                "datePublished": a.date,
                "url": a.url,
                "mainEntityOfPage": {"@type": "WebPage", "@id": a.url},
                "text": a.text,
                "dateModified": a.date_modified,
                "commentCount": a.comment_count,
            });
            if let Some(likes) = a.likes {
                p["interactionStatistic"] = json!([{
                    "@type": "InteractionCounter",
                    "interactionType": "https://schema.org/LikeAction",
                    "userInteractionCount": likes,
                }]);
            }
            p
        }
        SchemaKind::Profile(a) => {
            let mut person = json!({
                "@type": "Person",
                "name": a.name,
                "url": a.url,
                "description": a.description,
                "worksFor": a.works_for.map(|w| json!({"@type": "Organization", "name": w})),
                "image": a.image,
                "jobTitle": a.job_title,
            });
            if !a.same_as.is_empty() {
                person["sameAs"] = json!(a.same_as);
            }
            if !a.knows_about.is_empty() {
                person["knowsAbout"] = json!(a.knows_about);
            }
            json!({
                "@context": "https://schema.org",
                "@type": "ProfilePage",
                "mainEntity": person,
                "url": a.url,
            })
        }
        SchemaKind::Organization(a) => {
            let mut p = json!({
                "@context": "https://schema.org",
                "@type": "Organization",
                "name": a.name,
                "url": a.url,
                "logo": a.logo,
                "description": a.description,
            });
            if !a.same_as.is_empty() {
                p["sameAs"] = json!(a.same_as);
            }
            if a.telephone.is_some() || a.email.is_some() {
                p["contactPoint"] = json!({
                    "@type": "ContactPoint",
                    "contactType": "customer support",
                    "telephone": a.telephone,
                    "email": a.email,
                });
            }
            p
        }
        SchemaKind::LocalBusiness(a) => {
            let mut p = json!({
                "@context": "https://schema.org",
                "@type": a.business_type,
                "name": a.name,
                "url": a.url,
                "telephone": a.telephone,
            });
            if a.street.is_some() || a.city.is_some() || a.postal_code.is_some() {
                p["address"] = json!({
                    "@type": "PostalAddress",
                    "streetAddress": a.street,
                    "addressLocality": a.city,
                    "addressRegion": a.region,
                    "postalCode": a.postal_code,
                    "addressCountry": a.country,
                });
            }
            if let (Some(lat), Some(lon)) = (a.latitude, a.longitude) {
                p["geo"] = json!({"@type": "GeoCoordinates", "latitude": lat, "longitude": lon});
            }
            if !a.hours.is_empty() {
                p["openingHours"] = json!(a.hours);
            }
            if !a.same_as.is_empty() {
                p["sameAs"] = json!(a.same_as);
            }
            p
        }
    };

    let cleaned = strip_nulls(payload);
    let text = if indent == 0 {
        serde_json::to_string(&cleaned)?
    } else {
        let mut buf = Vec::new();
        let indent_bytes = " ".repeat(indent);
        let formatter = serde_json::ser::PrettyFormatter::with_indent(indent_bytes.as_bytes());
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
        serde::Serialize::serialize(&cleaned, &mut ser)?;
        String::from_utf8(buf).unwrap_or_default()
    };

    if script_tag {
        println!("<script type=\"application/ld+json\">");
        println!("{text}");
        println!("</script>");
    } else {
        println!("{text}");
    }
    OK
}

// --------------------------------------------------------------- validate

fn load_html(url: Option<&str>, file: Option<&str>) -> CmdResult<(String, String)> {
    match (file, url) {
        (Some(p), _) => Ok((std::fs::read_to_string(p)?, p.to_string())),
        (None, Some(u)) => {
            let rec = fetch_record(u, 30, true, None);
            match rec.error {
                Some(e) => err(e),
                None => Ok((rec.content.unwrap_or_default(), rec.url)),
            }
        }
        (None, None) => err("pass a URL or --file"),
    }
}

/// Required and recommended properties per type. "Required" means Google
/// will not show the rich result without it; "recommended" affects
/// eligibility for richer presentations and entity resolution.
fn requirements(schema_type: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match schema_type {
        "Article" | "NewsArticle" | "BlogPosting" => (
            &["headline"],
            &["author", "datePublished", "dateModified", "image", "publisher"],
        ),
        "Product" => (
            &["name"],
            &["image", "description", "offers", "brand", "aggregateRating", "review"],
        ),
        "Organization" => (&["name"], &["url", "logo", "sameAs", "contactPoint"]),
        "LocalBusiness" => (
            &["name", "address"],
            &["telephone", "openingHours", "geo", "priceRange", "url", "image"],
        ),
        "Person" => (&["name"], &["url", "sameAs", "jobTitle", "knowsAbout", "image"]),
        "FAQPage" => (&["mainEntity"], &[]),
        "BreadcrumbList" => (&["itemListElement"], &[]),
        "Event" => (&["name", "startDate", "location"], &["endDate", "offers", "performer", "image"]),
        "Recipe" => (
            &["name"],
            &["image", "recipeIngredient", "recipeInstructions", "author", "nutrition"],
        ),
        "VideoObject" => (
            &["name", "thumbnailUrl", "uploadDate"],
            &["description", "duration", "contentUrl", "embedUrl"],
        ),
        "SoftwareApplication" => (
            &["name"],
            &["applicationCategory", "operatingSystem", "offers", "aggregateRating"],
        ),
        "WebSite" => (&["name", "url"], &["potentialAction"]),
        "DiscussionForumPosting" => (
            &["headline", "author", "datePublished"],
            &["url", "text", "interactionStatistic"],
        ),
        "ProfilePage" => (&["mainEntity"], &["url", "dateCreated"]),
        _ => (&[], &[]),
    }
}

fn type_names(block: &Value) -> Vec<String> {
    match &block["@type"] {
        Value::String(s) => vec![s.rsplit('/').next().unwrap_or(s).to_string()],
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.rsplit('/').next().unwrap_or(s).to_string())
            .collect(),
        _ => vec![],
    }
}

pub fn validate(url: Option<&str>, file: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let (source, label) = load_html(url, file)?;
    let doc = scraper::Html::parse_document(&source);
    let blocks = html::extract_jsonld(&doc);

    let mut findings = Vec::new();
    let all_types = collect_types(&blocks);

    for (idx, block) in blocks.iter().enumerate() {
        let types = type_names(block);
        let mut issues: Vec<Value> = Vec::new();

        if block.get("@context").is_none() {
            issues.push(json!({
                "severity": "error",
                "message": "missing @context — parsers cannot resolve the vocabulary",
            }));
        }
        if types.is_empty() {
            issues.push(json!({"severity": "error", "message": "missing @type"}));
        }

        for t in &types {
            let (required, recommended) = requirements(t);
            for prop in required {
                if block.get(*prop).is_none() {
                    issues.push(json!({
                        "severity": "error",
                        "type": t,
                        "property": prop,
                        "message": format!("{t} requires `{prop}` for rich-result eligibility"),
                    }));
                }
            }
            for prop in recommended {
                if block.get(*prop).is_none() {
                    issues.push(json!({
                        "severity": "warning",
                        "type": t,
                        "property": prop,
                        "message": format!("{t} recommends `{prop}`"),
                    }));
                }
            }
        }

        findings.push(json!({
            "index": idx,
            "types": types,
            "issues": issues,
            "error_count": issues.iter().filter(|i| i["severity"] == "error").count(),
            "warning_count": issues.iter().filter(|i| i["severity"] == "warning").count(),
        }));
    }

    // Types that are no longer shown as Google Search rich results. Keeping
    // them is harmless for other consumers, so this is informational.
    let retired: Vec<&String> = all_types
        .iter()
        .filter(|t| matches!(t.as_str(), "FAQPage" | "HowTo" | "Dataset"))
        .collect();

    let errors: usize = findings
        .iter()
        .map(|f| f["error_count"].as_u64().unwrap_or(0) as usize)
        .sum();
    let warnings: usize = findings
        .iter()
        .map(|f| f["warning_count"].as_u64().unwrap_or(0) as usize)
        .sum();

    let result = json!({
        "url": label,
        "blocks_found": blocks.len(),
        "types": all_types,
        "retired_for_google_rich_results": retired,
        "errors": errors,
        "warnings": warnings,
        "findings": findings,
        "blocks": blocks,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("Source: {label}");
        println!("Schema blocks: {}", blocks.len());
        println!("Types: {}", all_types.join(", "));
        println!("Errors: {errors}   Warnings: {warnings}");
        for f in &findings {
            for i in f["issues"].as_array().unwrap() {
                println!(
                    "  [{}] {}",
                    i["severity"].as_str().unwrap_or("?"),
                    i["message"].as_str().unwrap_or_default()
                );
            }
        }
        if !retired.is_empty() {
            println!(
                "Note: {} no longer produce Google Search rich results.",
                retired
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    Ok(if errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

// ------------------------------------------------------- ecommerce (Product)

/// Merchant-listing requirements are stricter than plain Product rich
/// results: they need a price, a currency, and an availability value from
/// the schema.org enumeration.
const VALID_AVAILABILITY: &[&str] = &[
    "InStock",
    "OutOfStock",
    "PreOrder",
    "BackOrder",
    "Discontinued",
    "InStoreOnly",
    "LimitedAvailability",
    "OnlineOnly",
    "SoldOut",
    "PreSale",
];

fn offer_issues(offer: &Value, path: &str, issues: &mut Vec<Value>) {
    let mut push = |severity: &str, message: String| {
        issues.push(json!({"severity": severity, "path": path, "message": message}));
    };
    let has_price = offer.get("price").is_some()
        || offer.get("lowPrice").is_some()
        || offer.get("priceSpecification").is_some();
    if !has_price {
        push("error", "offer has no price / lowPrice / priceSpecification".into());
    }
    if offer.get("priceCurrency").is_none()
        && offer
            .get("priceSpecification")
            .and_then(|p| p.get("priceCurrency"))
            .is_none()
    {
        push("error", "offer has no priceCurrency (ISO 4217)".into());
    }
    match offer.get("availability").and_then(|v| v.as_str()) {
        None => push("error", "offer has no availability".into()),
        Some(a) => {
            let short = a.rsplit('/').next().unwrap_or(a);
            if !VALID_AVAILABILITY.contains(&short) {
                push(
                    "error",
                    format!("availability {short:?} is not a schema.org ItemAvailability value"),
                );
            }
        }
    }
    if offer.get("priceValidUntil").is_none() {
        push("warning", "offer has no priceValidUntil — listings can go stale".into());
    }
    if offer.get("shippingDetails").is_none() {
        push("warning", "offer has no shippingDetails — required for some merchant programs".into());
    }
    if offer.get("hasMerchantReturnPolicy").is_none() {
        push("warning", "offer has no hasMerchantReturnPolicy".into());
    }
}

pub fn ecommerce(url: Option<&str>, file: Option<&str>, json: bool) -> CmdResult<ExitCode> {
    let (source, label) = load_html(url, file)?;
    let doc = scraper::Html::parse_document(&source);
    let blocks = html::extract_jsonld(&doc);

    let products: Vec<&Value> = blocks
        .iter()
        .filter(|b| type_names(b).iter().any(|t| t == "Product" || t == "ProductGroup"))
        .collect();

    let mut issues: Vec<Value> = Vec::new();
    if products.is_empty() {
        issues.push(json!({
            "severity": "error",
            "path": "$",
            "message": "no Product schema found — merchant listings are not eligible",
        }));
    }

    for (i, product) in products.iter().enumerate() {
        let path = format!("$.product[{i}]");
        let mut push = |severity: &str, message: String| {
            issues.push(json!({"severity": severity, "path": path, "message": message}));
        };
        for prop in ["name", "image"] {
            if product.get(prop).is_none() {
                push("error", format!("Product is missing required `{prop}`"));
            }
        }
        let ids = ["gtin", "gtin8", "gtin12", "gtin13", "gtin14", "mpn", "sku", "isbn"];
        if !ids.iter().any(|k| product.get(*k).is_some()) {
            push(
                "error",
                "Product has no product identifier (gtin*/mpn/sku) — required for merchant listings"
                    .into(),
            );
        }
        if product.get("brand").is_none() {
            push("warning", "Product has no brand".into());
        }
        if product.get("description").is_none() {
            push("warning", "Product has no description".into());
        }
        if product.get("aggregateRating").is_none() && product.get("review").is_none() {
            push("warning", "Product has neither aggregateRating nor review".into());
        }

        match product.get("offers") {
            None => push("error", "Product has no offers".into()),
            Some(Value::Array(list)) => {
                for (j, offer) in list.iter().enumerate() {
                    offer_issues(offer, &format!("$.product[{i}].offers[{j}]"), &mut issues);
                }
            }
            Some(offer) => offer_issues(offer, &format!("$.product[{i}].offers"), &mut issues),
        }
    }

    let errors = issues.iter().filter(|i| i["severity"] == "error").count();
    let warnings = issues.iter().filter(|i| i["severity"] == "warning").count();

    let result = json!({
        "url": label,
        "products_found": products.len(),
        "merchant_listing_eligible": errors == 0 && !products.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "issues": issues,
    });

    if json {
        print_json(&result)?;
    } else {
        println!("Source: {label}");
        println!("Products found: {}", products.len());
        println!(
            "Merchant-listing eligible: {}",
            result["merchant_listing_eligible"]
        );
        for i in &issues {
            println!(
                "  [{}] {} — {}",
                i["severity"].as_str().unwrap_or("?"),
                i["path"].as_str().unwrap_or("$"),
                i["message"].as_str().unwrap_or_default()
            );
        }
    }

    Ok(if errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Extract the `@type` values present in a set of JSON-LD blocks.
pub fn collect_types(blocks: &[Value]) -> Vec<String> {
    let mut types: Vec<String> = blocks.iter().flat_map(type_names).collect();
    types.sort();
    types.dedup();
    types
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_without_identifier_fails() {
        let html = r#"<html><head><script type="application/ld+json">
        {"@context":"https://schema.org","@type":"Product","name":"X","image":"https://a/i.png",
         "offers":{"@type":"Offer","price":"9.99","priceCurrency":"USD","availability":"https://schema.org/InStock"}}
        </script></head><body></body></html>"#;
        let doc = scraper::Html::parse_document(html);
        let blocks = html::extract_jsonld(&doc);
        assert_eq!(collect_types(&blocks), vec!["Product"]);
    }

    #[test]
    fn strip_nulls_removes_empty_members() {
        let v = json!({"a": 1, "b": null, "c": {"d": null, "e": 2}});
        assert_eq!(strip_nulls(v), json!({"a": 1, "c": {"e": 2}}));
    }
}
