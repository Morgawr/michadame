use super::deinflect::parse_rule_flags;
use super::models::{DeinflectionCandidate, DictMetadata, GlossaryEntry, TermEntry};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::Path;

pub struct DictDatabase {
    pub conn: Connection,
}

impl DictDatabase {
    /// Opens an existing dictionary SQLite database in read-only mode for lookups.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .context("Failed to open dictionary database")?;

        // Fast read pragma
        let _ = conn.execute_batch("PRAGMA query_only = ON; PRAGMA cache_size = -64000;");

        Ok(Self { conn })
    }

    /// Creates or connects to a dictionary SQLite database for reading/writing.
    pub fn open_or_create<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open/create dictionary database")?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT
            );
            CREATE TABLE IF NOT EXISTS terms (
                term TEXT,
                reading TEXT,
                definition_tags TEXT,
                rules TEXT,
                score REAL,
                glossary TEXT,
                sequence INT,
                term_tags TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_terms_term ON terms(term);
            CREATE INDEX IF NOT EXISTS idx_terms_reading ON terms(reading);
            ",
        )?;
        Ok(())
    }

    /// Loads the stored dictionary metadata from the `metadata` table.
    pub fn get_metadata(&self) -> Result<DictMetadata> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM metadata")?;
        let rows = stmt.query_map([], |row| {
            let key: String = row.get(0)?;
            let value: String = row.get(1)?;
            Ok((key, value))
        })?;

        let mut map: HashMap<String, String> = HashMap::new();
        for r in rows {
            if let Ok((k, v)) = r {
                map.insert(k, v);
            }
        }

        let total_entries: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM terms", [], |r| r.get(0))
            .unwrap_or(0);

        Ok(DictMetadata {
            title: map.remove("title").unwrap_or_else(|| "Unknown Dictionary".to_string()),
            revision: map.remove("revision").unwrap_or_default(),
            author: map.remove("author"),
            description: map.remove("description"),
            download_url: map.remove("downloadUrl"),
            index_url: map.remove("indexUrl"),
            source_language: map.remove("sourceLanguage"),
            target_language: map.remove("targetLanguage"),
            total_entries,
        })
    }

    /// Looks up terms corresponding to candidate words and deinflections.
    pub fn find_terms(&self, candidates: &[DeinflectionCandidate]) -> Result<Vec<TermEntry>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self.conn.prepare_cached(
            "SELECT term, reading, definition_tags, rules, score, glossary, sequence, term_tags 
             FROM terms 
             WHERE term = ?1 OR reading = ?1 OR term = ?2 OR reading = ?2
             ORDER BY score DESC LIMIT 20",
        )?;

        let mut entries = Vec::new();
        let mut seen_keys = HashSet::new();

        for candidate in candidates {
            let alt_kana = to_alternate_kana(&candidate.term);
            let alt = alt_kana.as_deref().unwrap_or(&candidate.term);
            let mut rows = stmt.query(params![candidate.term, alt])?;

            while let Some(row) = rows.next()? {
                let term: String = row.get(0)?;
                let reading: String = row.get(1)?;
                let definition_tags: Option<String> = row.get(2)?;
                let rules: String = row.get(3)?;
                let score: f64 = row.get(4)?;
                let glossary_raw: String = row.get(5)?;
                let sequence: i64 = row.get(6)?;
                let term_tags: Option<String> = row.get(7)?;

                // If term was deinflected, verify rule compatibility
                if candidate.rules != 0 {
                    let rule_flags = parse_rule_flags(&rules);
                    if (rule_flags & candidate.rules) == 0 {
                        continue;
                    }
                }

                let key = (term.clone(), reading.clone(), sequence);
                if !seen_keys.insert(key) {
                    continue;
                }

                // Parse glossary elements
                let glossary = parse_glossary_json(&glossary_raw);

                entries.push(TermEntry {
                    term,
                    reading,
                    definition_tags,
                    rules,
                    score,
                    glossary,
                    sequence,
                    term_tags,
                    inflection_reasons: candidate.reasons.clone(),
                });
            }
        }

        // Sort: verbatim matches first, then by score descending
        entries.sort_by(|a, b| {
            let a_verbatim = a.inflection_reasons.is_empty();
            let b_verbatim = b.inflection_reasons.is_empty();
            match (a_verbatim, b_verbatim) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal),
            }
        });

        Ok(entries)
    }

    /// Indexes a Yomitan zip archive into an SQLite database file.
    pub fn build_database_from_zip<F, P1: AsRef<Path>, P2: AsRef<Path>>(
        zip_path: P1,
        db_path: P2,
        mut progress_cb: F,
    ) -> Result<DictMetadata>
    where
        F: FnMut(&str, f32),
    {
        let zip_file = File::open(&zip_path).context("Failed to open dictionary zip archive")?;
        let mut archive = zip::ZipArchive::new(zip_file).context("Failed to parse zip archive")?;

        let tmp_db_path = format!("{}.tmp", db_path.as_ref().display());
        let _ = std::fs::remove_file(&tmp_db_path);

        let mut conn = Connection::open(&tmp_db_path)
            .context("Failed to create temporary SQLite database")?;

        // Optimization pragmas during bulk import
        conn.execute_batch(
            "
            PRAGMA synchronous = OFF;
            PRAGMA journal_mode = MEMORY;
            PRAGMA temp_store = MEMORY;
            PRAGMA cache_size = -128000;
            ",
        )?;

        Self::init_schema(&conn)?;

        // 1. Read index.json
        progress_cb("Reading dictionary metadata...", 0.05);
        let mut meta = DictMetadata::default();
        if let Ok(mut index_file) = archive.by_name("index.json") {
            if let Ok(val) = serde_json::from_reader::<_, serde_json::Value>(&mut index_file) {
                let tx = conn.transaction()?;
                {
                    let mut stmt = tx.prepare(
                        "INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)",
                    )?;
                    if let Some(obj) = val.as_object() {
                        for (k, v) in obj {
                            let val_str = if let Some(s) = v.as_str() {
                                s.to_string()
                            } else {
                                v.to_string()
                            };
                            stmt.execute(params![k, val_str])?;
                        }
                    }
                }
                tx.commit()?;
                meta = serde_json::from_value(val).unwrap_or_default();
            }
        }

        // 2. Discover term_bank_*.json files
        let mut term_bank_names = Vec::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            let name = file.name().to_string();
            // Match files like "term_bank_1.json" or "jitendex/term_bank_1.json"
            if name.ends_with(".json") && name.contains("term_bank_") {
                term_bank_names.push(name);
            }
        }
        term_bank_names.sort();

        let total_banks = term_bank_names.len();
        if total_banks == 0 {
            anyhow::bail!("No term bank files found in dictionary archive");
        }

        // 3. Process each term bank in transactions
        let mut total_inserted = 0;
        let mut insert_batch: Vec<(String, String, String, String, f64, String, i64, String)> =
            Vec::with_capacity(5000);

        for (idx, name) in term_bank_names.iter().enumerate() {
            let progress = 0.05 + 0.85 * (idx as f32 / total_banks as f32);
            progress_cb(
                &format!("Importing term bank {}/{}...", idx + 1, total_banks),
                progress,
            );

            let mut file = archive.by_name(name)?;
            let bank_entries: serde_json::Value = serde_json::from_reader(&mut file)
                .with_context(|| format!("Failed to parse JSON in {name}"))?;

            if let Some(arr) = bank_entries.as_array() {
                for item in arr {
                    // Item schema: [term, reading, definition_tags, rules, score, glossary, sequence, term_tags]
                    if let Some(fields) = item.as_array() {
                        if fields.len() >= 6 {
                            let term = fields[0].as_str().unwrap_or("").to_string();
                            let reading = fields[1].as_str().unwrap_or("").to_string();
                            let def_tags = fields[2].as_str().unwrap_or("").to_string();
                            let rules = fields[3].as_str().unwrap_or("").to_string();
                            let score = fields[4].as_f64().unwrap_or(0.0);
                            let glossary = serde_json::to_string(&fields[5]).unwrap_or_default();
                            let seq = fields.get(6).and_then(|v| v.as_i64()).unwrap_or(0);
                            let term_tags = fields.get(7).and_then(|v| v.as_str()).unwrap_or("").to_string();

                            insert_batch.push((
                                term, reading, def_tags, rules, score, glossary, seq, term_tags,
                            ));
                            total_inserted += 1;
                        }
                    }
                }
            }

            if insert_batch.len() >= 5000 {
                let tx = conn.transaction()?;
                {
                    let mut stmt = tx.prepare(
                        "INSERT INTO terms (term, reading, definition_tags, rules, score, glossary, sequence, term_tags)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
                    )?;
                    for row in insert_batch.drain(..) {
                        stmt.execute(params![
                            row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7
                        ])?;
                    }
                }
                tx.commit()?;
            }
        }

        // Flush remainder
        if !insert_batch.is_empty() {
            let tx = conn.transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO terms (term, reading, definition_tags, rules, score, glossary, sequence, term_tags)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
                )?;
                for row in insert_batch.drain(..) {
                    stmt.execute(params![
                        row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7
                    ])?;
                }
            }
            tx.commit()?;
        }

        // 4. Build indexes
        progress_cb("Building lookup indexes...", 0.93);
        conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_terms_term ON terms(term);
            CREATE INDEX IF NOT EXISTS idx_terms_reading ON terms(reading);
            ",
        )?;

        // Close connection cleanly
        drop(conn);

        // Atomically replace target database
        progress_cb("Finalizing dictionary...", 0.98);
        if let Some(parent) = db_path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&tmp_db_path, db_path.as_ref())
            .context("Failed to atomically move new dictionary database into place")?;

        meta.total_entries = total_inserted;
        progress_cb("Dictionary ready!", 1.0);

        Ok(meta)
    }
}

/// Converts Katakana to Hiragana (e.g. "ラーメン" -> "らーめん", "ダメ" -> "だめ")
pub fn katakana_to_hiragana(s: &str) -> String {
    s.chars()
        .map(|c| {
            let u = c as u32;
            if (0x30A1..=0x30F6).contains(&u) {
                std::char::from_u32(u - 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// Converts Hiragana to Katakana (e.g. "なにか" -> "ナニカ", "だめ" -> "ダメ")
pub fn hiragana_to_katakana(s: &str) -> String {
    s.chars()
        .map(|c| {
            let u = c as u32;
            if (0x3041..=0x3096).contains(&u) {
                std::char::from_u32(u + 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// Returns the alternate kana representation if the string contains Hiragana or Katakana.
pub fn to_alternate_kana(s: &str) -> Option<String> {
    let has_kata = s.chars().any(|c| (0x30A1..=0x30F6).contains(&(c as u32)));
    let has_hira = s.chars().any(|c| (0x3041..=0x3096).contains(&(c as u32)));
    if has_kata {
        Some(katakana_to_hiragana(s))
    } else if has_hira {
        Some(hiragana_to_katakana(s))
    } else {
        None
    }
}

/// Parses the raw JSON glossary string into `GlossaryEntry` items.
pub fn parse_glossary_json(raw: &str) -> Vec<GlossaryEntry> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) else {
        return vec![GlossaryEntry::Text(raw.to_string())];
    };

    match val {
        serde_json::Value::Array(arr) => arr
            .into_iter()
            .map(|item| match item {
                serde_json::Value::String(s) => GlossaryEntry::Text(s),
                structured => GlossaryEntry::Structured(structured),
            })
            .collect(),
        serde_json::Value::String(s) => vec![GlossaryEntry::Text(s)],
        other => vec![GlossaryEntry::Structured(other)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_database_crud_and_query() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let db = DictDatabase::open_or_create(&db_path).unwrap();
        db.conn
            .execute(
                "INSERT INTO metadata (key, value) VALUES ('title', 'Test Jitendex'), ('revision', '2026.08.11.0')",
                [],
            )
            .unwrap();

        db.conn
            .execute(
                "INSERT INTO terms VALUES ('食べる', 'たべる', 'v1 vt', 'v1', 200.0, '[\"to eat\"]', 100, '')",
                [],
            )
            .unwrap();

        let meta = db.get_metadata().unwrap();
        assert_eq!(meta.title, "Test Jitendex");
        assert_eq!(meta.revision, "2026.08.11.0");
        assert_eq!(meta.total_entries, 1);

        // Test finding verbatim term
        let cands = vec![DeinflectionCandidate {
            term: "食べる".to_string(),
            rules: 0,
            reasons: vec![],
        }];
        let results = db.find_terms(&cands).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].term, "食べる");
        assert_eq!(results[0].reading, "たべる");

        // Test finding deinflected term matching rules
        let cands = vec![DeinflectionCandidate {
            term: "食べる".to_string(),
            rules: super::super::deinflect::RULE_V1,
            reasons: vec!["past".to_string()],
        }];
        let results = db.find_terms(&cands).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].inflection_reasons, vec!["past"]);

        // Test finding term with non-matching rules (should reject)
        let cands = vec![DeinflectionCandidate {
            term: "食べる".to_string(),
            rules: super::super::deinflect::RULE_V5, // wrong rule!
            reasons: vec!["past".to_string()],
        }];
        let results = db.find_terms(&cands).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_build_database_from_zip() {
        use std::io::Write;
        let dir = tempdir().unwrap();
        let zip_path = dir.path().join("dict.zip");
        let db_path = dir.path().join("out.db");

        // Create a mock Yomitan zip file
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            zip.start_file("index.json", options).unwrap();
            let index_json = serde_json::json!({
                "title": "Jitendex Mini",
                "revision": "2026.10.02",
                "format": 3
            });
            zip.write_all(index_json.to_string().as_bytes()).unwrap();

            zip.start_file("term_bank_1.json", options).unwrap();
            let bank_json = serde_json::json!([
                ["走る", "はしる", "v5r", "v5", 180.0, ["to run"], 1, ""],
                ["読む", "よむ", "v5m", "v5", 190.0, ["to read"], 2, ""]
            ]);
            zip.write_all(bank_json.to_string().as_bytes()).unwrap();

            zip.finish().unwrap();
        }

        let meta = DictDatabase::build_database_from_zip(&zip_path, &db_path, |_, _| {}).unwrap();
        assert_eq!(meta.title, "Jitendex Mini");
        assert_eq!(meta.revision, "2026.10.02");
        assert_eq!(meta.total_entries, 2);

        // Open newly indexed database and query
        let db = DictDatabase::open(&db_path).unwrap();
        let cands = vec![DeinflectionCandidate {
            term: "走る".to_string(),
            rules: 0,
            reasons: vec![],
        }];
        let results = db.find_terms(&cands).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].term, "走る");
        assert_eq!(results[0].reading, "はしる");
    }
}
