//! Bookmarks and history readers (Chromium JSON/SQLite, Firefox places.sqlite).

use crate::browsers::{meta, Family};
use arclet_sdk::{Error, Result, Value};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub struct Item {
    pub title: String,
    pub url: String,
}

/// Run `sql` (columns: url, title) against a *copy* of the DB: browsers hold their
/// profile databases locked while running, and the WAL is not needed for recent rows.
fn query_rows(file: Option<&Path>, sql: &str) -> Result<Vec<Item>> {
    let file = file.filter(|f| f.exists()).ok_or_else(|| Error::msg("Browser data file was not found"))?;
    let tmp = std::env::temp_dir().join(format!("arclet-browser-{}-{}", std::process::id(), file.file_name().unwrap().to_string_lossy()));
    std::fs::copy(file, &tmp)?;
    let result = (|| -> rusqlite::Result<Vec<Item>> {
        let db = Connection::open_with_flags(&tmp, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut stmt = db.prepare(sql)?;
        let rows = stmt.query_map([], |r| Ok(Item { url: r.get(0)?, title: r.get(1)? }))?.collect();
        rows
    })();
    let _ = std::fs::remove_file(&tmp);
    result.map_err(|e| Error::msg(e.to_string()))
}

fn collect_chrome_bookmarks(node: &Value, acc: &mut Vec<(i64, Item)>) {
    if node["type"] == "url" {
        if let Some(url) = node["url"].as_str().filter(|u| !u.is_empty()) {
            let title = node["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(url);
            let added = node["date_added"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
            acc.push((added, Item { title: title.into(), url: url.into() }));
        }
    }
    for child in node["children"].as_array().into_iter().flatten() {
        collect_chrome_bookmarks(child, acc);
    }
}

pub fn bookmarks(browser: &str) -> Result<Vec<Item>> {
    let m = meta(browser);
    if m.family == Family::Chromium {
        let path = m.bookmarks_path.filter(|p| p.exists()).ok_or_else(|| Error::msg("Chrome bookmarks file was not found"))?;
        let parsed: Value = serde_json_from(&std::fs::read_to_string(path)?)?;
        let mut items = vec![];
        for root in parsed["roots"].as_object().into_iter().flat_map(|o| o.values()) {
            collect_chrome_bookmarks(root, &mut items);
        }
        items.sort_by(|a, b| b.0.cmp(&a.0));
        return Ok(items.into_iter().take(16).map(|(_, i)| i).collect());
    }
    query_rows(
        m.bookmarks_path.as_deref(),
        "SELECT p.url AS url, COALESCE(NULLIF(b.title, ''), p.url) AS title
           FROM moz_bookmarks b JOIN moz_places p ON p.id = b.fk
          WHERE b.type = 1 AND p.url IS NOT NULL
          ORDER BY b.dateAdded DESC LIMIT 16",
    )
}

pub fn history(browser: &str) -> Result<Vec<Item>> {
    let m = meta(browser);
    if m.family == Family::Chromium {
        return query_rows(
            m.history_path.as_deref(),
            "SELECT url, COALESCE(NULLIF(title, ''), url) AS title FROM urls
              WHERE hidden = 0 ORDER BY last_visit_time DESC LIMIT 16",
        );
    }
    query_rows(
        m.history_path.as_deref(),
        "SELECT url, COALESCE(NULLIF(title, ''), url) AS title FROM moz_places
          WHERE hidden = 0 AND visit_count > 0 AND url IS NOT NULL
          ORDER BY last_visit_date DESC LIMIT 16",
    )
}

fn serde_json_from(text: &str) -> Result<Value> {
    text.parse::<Value>().map_err(|e| Error::msg(e.to_string()))
}
