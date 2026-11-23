//! What the tokens that sandboxes use cost: a price list, limits per key, and
//! the report behind the Usage page.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Prices of one model or family, in US dollars per million tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Price {
    /// A model name, with `*` for any run of characters: `claude-opus-5-5*`.
    pub model: String,
    pub input: f64,
    pub output: f64,
    /// Input read from the provider's cache.
    #[serde(default)]
    pub cache_read: f64,
    /// Input written to the cache; where a provider does not charge for it apart, the input price.
    #[serde(default)]
    pub cache_write: f64,
}

/// A limit per day (UTC) on one key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Limit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_tokens: Option<u64>,
}

/// List prices as the providers published them on 2026-11-23, standard tier,
/// for prompts of ordinary length. They change; the list can be edited under
/// Usage. More specific patterns come first, because the first match counts.
pub fn default_prices() -> Vec<Price> {
    // (pattern, input, cache read, cache write, output)
    const LIST: &[(&str, f64, f64, f64, f64)] = &[
        ("claude-fable-5-2*", 8.0, 0.20, 10.0, 40.0),
        ("claude-mythos-5-2*", 8.0, 0.20, 10.0, 40.0),
        ("claude-fable-5-1*", 10.0, 0.25, 12.5, 50.0),
        ("claude-fable-5*", 10.0, 1.0, 12.5, 50.0),
        ("claude-opus-5-5*", 4.0, 0.20, 5.0, 20.0),
        ("claude-opus-5*", 5.0, 0.50, 6.25, 25.0),
        ("claude-opus-4-1*", 15.0, 1.50, 18.75, 75.0),
        ("claude-opus-4-5*", 5.0, 0.50, 6.25, 25.0),
        ("claude-opus-4-6*", 5.0, 0.50, 6.25, 25.0),
        ("claude-opus-4-7*", 5.0, 0.50, 6.25, 25.0),
        ("claude-opus-4-8*", 5.0, 0.50, 6.25, 25.0),
        ("claude-opus-4*", 15.0, 1.50, 18.75, 75.0),
        ("claude-sonnet-5-5*", 2.0, 0.10, 2.5, 10.0),
        ("claude-sonnet-5*", 2.0, 0.20, 2.5, 10.0),
        ("claude-sonnet-4*", 3.0, 0.30, 3.75, 15.0),
        ("claude-haiku-5-5*", 0.10, 0.01, 0.125, 0.50),
        ("claude-haiku-4-5*", 1.0, 0.10, 1.25, 5.0),
        ("gpt-6.2-astra*", 8.0, 0.40, 8.0, 40.0),
        ("gpt-6.2-sol*", 2.0, 0.10, 2.0, 10.0),
        ("gpt-6-astra*", 10.0, 1.0, 10.0, 50.0),
        ("gpt-6.1-sol*", 2.0, 0.10, 2.0, 10.0),
        ("gpt-6.1-terra*", 1.0, 0.05, 1.0, 5.0),
        ("gpt-6-luna*", 0.10, 0.01, 0.10, 0.50),
        ("gpt-6-sol*", 2.0, 0.20, 2.0, 10.0),
        ("gpt-5.6-sol*", 4.0, 0.40, 4.0, 20.0),
        ("gpt-5.6-terra*", 2.0, 0.20, 2.0, 12.0),
        ("gpt-5.6-luna*", 0.20, 0.02, 0.20, 1.20),
        ("gpt-5.5*", 5.0, 0.50, 5.0, 30.0),
        ("gpt-5.4-mini*", 0.75, 0.075, 0.75, 4.50),
        ("gpt-5.4-nano*", 0.20, 0.02, 0.20, 1.25),
        ("gpt-5.4*", 2.5, 0.25, 2.5, 15.0),
        ("gpt-5.3-codex*", 1.75, 0.175, 1.75, 14.0),
        ("o3-pro*", 20.0, 20.0, 20.0, 80.0),
        ("o3*", 2.0, 0.50, 2.0, 8.0),
        ("gemini-4-neon*", 3.0, 0.15, 3.0, 15.0),
        ("gemini-4-hydrogen*", 1.5, 0.075, 1.5, 7.5),
        ("gemini-4-*", 2.0, 0.10, 2.0, 10.0),
        ("gemini-3.8-flash*", 0.75, 0.075, 0.75, 3.75),
        ("gemini-3.6-flash*", 0.75, 0.075, 0.75, 3.75),
        ("gemini-3.5-flash-lite*", 0.30, 0.03, 0.30, 2.50),
    ];
    LIST.iter()
        .map(|(model, input, cache_read, cache_write, output)| Price {
            model: model.to_string(),
            input: *input,
            output: *output,
            cache_read: *cache_read,
            cache_write: *cache_write,
        })
        .collect()
}

/// Whether `name` fits a pattern in which `*` stands for any run of characters.
fn fits(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else { return false };
    let parts: Vec<&str> = parts.collect();
    for (at, part) in parts.iter().enumerate() {
        if at == parts.len() - 1 {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(found) => rest = &rest[found + part.len()..],
            None => return false,
        }
    }
    rest.is_empty()
}

pub fn tokens(row: &Value) -> u64 {
    ["input", "output", "cache_read", "cache_write"].iter().map(|key| row[key].as_u64().unwrap_or(0)).sum()
}

/// What a row of usage costs in dollars; `None` when no price fits its model.
pub fn cost(row: &Value, prices: &[Price]) -> Option<f64> {
    let model = row["model"].as_str().unwrap_or_default().to_lowercase();
    let price = prices.iter().find(|price| fits(&price.model.to_lowercase(), &model))?;
    let of = |key: &str, per_million: f64| row[key].as_u64().unwrap_or(0) as f64 * per_million / 1e6;
    Some(of("input", price.input) + of("output", price.output) + of("cache_read", price.cache_read) + of("cache_write", price.cache_write))
}

/// The usage rows with their cost, sums per key, sandbox and model, and each
/// limited key's standing today.
pub fn report(rows: Vec<Value>, prices: &[Price], limits: &BTreeMap<String, Limit>, today: &BTreeMap<String, (u64, f64)>, bucket_ms: u64) -> Value {
    let mut unpriced: Vec<String> = Vec::new();
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|mut row| {
            let dollars = cost(&row, prices);
            if dollars.is_none() {
                let model = row["model"].as_str().unwrap_or_default().to_string();
                if !unpriced.contains(&model) {
                    unpriced.push(model);
                }
            }
            row["cost"] = json!(dollars);
            row
        })
        .collect();
    let sums = |key: &str| -> Vec<Value> {
        let mut by: BTreeMap<String, (u64, f64, u64)> = BTreeMap::new();
        for row in &rows {
            let entry = by.entry(row[key].as_str().unwrap_or_default().to_string()).or_default();
            entry.0 += tokens(row);
            entry.1 += row["cost"].as_f64().unwrap_or(0.0);
            entry.2 += row["requests"].as_u64().unwrap_or(0);
        }
        let mut list: Vec<Value> = by
            .into_iter()
            .map(|(name, (tokens, cost, requests))| json!({ "name": name, "tokens": tokens, "cost": cost, "requests": requests }))
            .collect();
        list.sort_by(|a, b| b["tokens"].as_u64().cmp(&a["tokens"].as_u64()));
        list
    };
    let keys: Vec<Value> = limits
        .iter()
        .map(|(key, limit)| {
            let (tokens, dollars) = today.get(key).copied().unwrap_or((0, 0.0));
            let reached = limit.daily_tokens.is_some_and(|max| tokens >= max) || limit.daily_usd.is_some_and(|max| dollars >= max);
            json!({ "key": key, "daily_usd": limit.daily_usd, "daily_tokens": limit.daily_tokens, "tokens_today": tokens, "cost_today": dollars, "reached": reached })
        })
        .collect();
    json!({
        "bucket_ms": bucket_ms,
        "rows": rows,
        "by_key": sums("secret"), "by_sandbox": sums("sandbox"), "by_model": sums("model"),
        "limits": keys,
        "unpriced": unpriced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_fitting_price_counts() {
        let prices = default_prices();
        let row = json!({ "model": "claude-opus-5-5-20261101", "input": 1_000_000, "output": 100_000, "cache_read": 2_000_000, "cache_write": 0 });
        assert_eq!(cost(&row, &prices), Some(4.0 + 2.0 + 0.4));
        assert_eq!(cost(&json!({ "model": "claude-opus-4-20250514", "input": 1_000_000 }), &prices), Some(15.0));
        assert_eq!(cost(&json!({ "model": "something-else", "input": 5 }), &prices), None);
        assert!(fits("gpt-*-mini*", "gpt-5.4-mini-2026") && !fits("gpt-5.4", "gpt-5.4-mini"));
    }
}
