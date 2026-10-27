use super::deinflect::parse_rule_flags;
use super::models::{DeinflectionCandidate, DictMetadata, GlossaryEntry, TermEntry};
use super::particles::ParticleIndex;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

static DB_GENERATION: AtomicU64 = AtomicU64::new(1);

pub struct DictDatabase {
    pub conn: Connection,
    /// Unique id per opened database instance (used to invalidate lookup caches when the
    /// dictionary is swapped out, e.g. after a sync).
    pub generation: u64,
    /// Particle-swap index for expressions (built lazily in the background).
    particle_index: Arc<RwLock<Option<Arc<ParticleIndex>>>>,
}

impl DictDatabase {
    fn from_conn(conn: Connection) -> Self {
        Self {
            conn,
            generation: DB_GENERATION.fetch_add(1, Ordering::Relaxed),
            particle_index: Arc::new(RwLock::new(None)),
        }
    }

    /// Opens an existing dictionary SQLite database in read-only mode for lookups.
    /// Also kicks off a background build of the particle-swap index using a separate
    /// read-only connection so the UI thread is never blocked.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Self::open_read_only_conn(path.as_ref())?;
        let db = Self::from_conn(conn);
        super::lookup::invalidate_scan_cache();

        let slot = db.particle_index.clone();
        let path_buf = path.as_ref().to_path_buf();
        std::thread::Builder::new()
            .name("dict-particle-index".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let result = Self::open_read_only_conn(&path_buf)
                    .map(Self::from_conn)
                    .and_then(|bg_db| ParticleIndex::build(&bg_db));
                match result {
                    Ok(index) => {
                        tracing::info!(
                            "Built particle-swap index with {} expressions in {:?}",
                            index.len(),
                            started.elapsed()
                        );
                        if let Ok(mut lock) = slot.write() {
                            *lock = Some(Arc::new(index));
                        }
                    }
                    Err(e) => tracing::warn!("Failed to build particle-swap index: {e}"),
                }
            })
            .ok();

        Ok(db)
    }

    fn open_read_only_conn(path: &Path) -> Result<Connection> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .context("Failed to open dictionary database")?;

        // Fast read pragma
        let _ = conn.execute_batch("PRAGMA query_only = ON; PRAGMA cache_size = -64000;");
        Ok(conn)
    }

    /// Creates or connects to a dictionary SQLite database for reading/writing.
    pub fn open_or_create<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open/create dictionary database")?;
        Self::init_schema(&conn)?;
        Ok(Self::from_conn(conn))
    }

    /// Returns the particle-swap index if it has finished building.
    pub fn particle_index(&self) -> Option<Arc<ParticleIndex>> {
        self.particle_index.read().ok().and_then(|g| g.clone())
    }

    /// Synchronously (re)builds the particle-swap index from this connection.
    pub fn rebuild_particle_index(&self) -> Result<()> {
        let index = ParticleIndex::build(self)?;
        if let Ok(mut lock) = self.particle_index.write() {
            *lock = Some(Arc::new(index));
        }
        Ok(())
    }

    fn init_tables(conn: &Connection) -> Result<()> {
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
            ",
        )?;
        Ok(())
    }

    fn init_indexes(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_terms_term ON terms(term);
            CREATE INDEX IF NOT EXISTS idx_terms_reading ON terms(reading);
            ",
        )?;
        Ok(())
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        Self::init_tables(conn)?;
        Self::init_indexes(conn)?;
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
            title: map
                .remove("title")
                .unwrap_or_else(|| "Unknown Dictionary".to_string()),
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
             ORDER BY score DESC LIMIT 100",
        )?;

        let mut entries = Vec::new();
        let mut seen_keys = HashSet::new();

        for candidate in candidates {
            let alt_kana = to_alternate_kana(&candidate.term);
            let alt = alt_kana.as_deref().unwrap_or(&candidate.term);
            let mut rows = stmt.query(params![candidate.term, alt])?;
            let mut matched_for_candidate = 0usize;

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
                    frequency: None,
                    pitches: Vec::new(),
                    custom_name_id: None,
                    custom_tag: None,
                });
                matched_for_candidate += 1;
                if matched_for_candidate >= 20 {
                    break;
                }
            }
        }

        // Sort: verbatim matches first, then by score descending
        entries.sort_by(|a, b| {
            let a_verbatim = a.inflection_reasons.is_empty();
            let b_verbatim = b.inflection_reasons.is_empty();
            match (a_verbatim, b_verbatim) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => b
                    .score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            }
        });

        Ok(entries)
    }

    /// Indexes a Yomitan zip archive into an SQLite database file (alias for `build_database_from_zip`).
    pub fn import_zip<F, P1: AsRef<Path>, P2: AsRef<Path>>(
        zip_path: P1,
        db_path: P2,
        progress_cb: F,
    ) -> Result<DictMetadata>
    where
        F: FnMut(&str, f32),
    {
        Self::build_database_from_zip(zip_path, db_path, progress_cb)
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

        // 1. Locate and validate index.json before modifying disk state
        progress_cb("Reading dictionary metadata...", 0.05);
        let index_entry_name = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|name| name == "index.json" || name.ends_with("/index.json"))
            .ok_or_else(|| {
                anyhow::anyhow!("Invalid Yomitan dictionary: missing index.json in archive")
            })?;

        let index_val: serde_json::Value = {
            let mut index_file = archive.by_name(&index_entry_name)?;
            serde_json::from_reader(&mut index_file)
                .with_context(|| format!("Failed to parse JSON in {index_entry_name}"))?
        };

        let index_obj = index_val
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Invalid index.json: root must be a JSON object"))?;

        let title = index_obj
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("Invalid index.json: missing or empty dictionary 'title'")
            })?
            .to_string();

        if let Some(fmt_val) = index_obj.get("format").or_else(|| index_obj.get("version")) {
            let fmt = fmt_val.as_i64().ok_or_else(|| {
                anyhow::anyhow!("Invalid index.json: 'format'/'version' must be an integer")
            })?;
            if !(1..=3).contains(&fmt) {
                anyhow::bail!("Unsupported Yomitan dictionary format version: {fmt}");
            }
        }

        // 2. Discover term_bank_*.json files
        let mut term_bank_names = Vec::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            let name = file.name().to_string();
            // Match files like "term_bank_1.json" or "jitendex/term_bank_1.json"
            if name.ends_with(".json")
                && name.contains("term_bank_")
                && !name.contains("term_meta_bank_")
            {
                term_bank_names.push(name);
            }
        }
        term_bank_names.sort_by(|a, b| {
            extract_term_bank_number(a)
                .cmp(&extract_term_bank_number(b))
                .then_with(|| a.cmp(b))
        });

        let total_banks = term_bank_names.len();
        if total_banks == 0 {
            anyhow::bail!("No term bank files found in dictionary archive");
        }

        if let Some(parent) = db_path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_db_path = db_path.as_ref().with_extension("db.tmp");
        let _ = std::fs::remove_file(&tmp_db_path);

        let import_result = (|| -> Result<DictMetadata> {
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

            Self::init_tables(&conn)?;

            let mut meta: DictMetadata =
                serde_json::from_value(index_val.clone()).unwrap_or_default();
            meta.title = title;
            {
                let tx = conn.transaction()?;
                {
                    let mut stmt =
                        tx.prepare("INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)")?;
                    for (k, v) in index_obj {
                        let val_str = if let Some(s) = v.as_str() {
                            s.to_string()
                        } else {
                            v.to_string()
                        };
                        stmt.execute(params![k, val_str])?;
                    }
                }
                tx.commit()?;
            }

            // 3. Process and validate each term bank in transactions
            let mut total_inserted = 0usize;
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

                let arr = bank_entries.as_array().ok_or_else(|| {
                    anyhow::anyhow!("Invalid term bank {name}: root must be a JSON array")
                })?;

                if arr.is_empty() {
                    anyhow::bail!("Invalid term bank {name}: term bank array is empty");
                }

                for (row_idx, item) in arr.iter().enumerate() {
                    let row = parse_and_validate_term_row(item, name, row_idx)?;
                    insert_batch.push(row);
                    total_inserted += 1;

                    if insert_batch.len() >= 5000 {
                        flush_term_batch(&mut conn, &mut insert_batch)?;
                    }
                }
            }

            if !insert_batch.is_empty() {
                flush_term_batch(&mut conn, &mut insert_batch)?;
            }

            if total_inserted == 0 {
                anyhow::bail!("Dictionary archive contains zero valid term entries");
            }

            // 4. Build indexes
            progress_cb("Building lookup indexes...", 0.93);
            Self::init_indexes(&conn)?;

            // 5. Structural verification before publication
            let db_count: usize = conn.query_row("SELECT COUNT(*) FROM terms", [], |r| r.get(0))?;
            if db_count != total_inserted || db_count == 0 {
                anyhow::bail!(
                    "Temporary dictionary verification failed: expected {} rows, found {}",
                    total_inserted,
                    db_count
                );
            }
            let title_count: usize = conn.query_row(
                "SELECT COUNT(*) FROM metadata WHERE key = 'title' AND length(trim(value)) > 0",
                [],
                |r| r.get(0),
            )?;
            if title_count == 0 {
                anyhow::bail!("Temporary dictionary verification failed: missing title metadata");
            }
            let integrity: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
            if !integrity.eq_ignore_ascii_case("ok") {
                anyhow::bail!(
                    "Temporary dictionary failed SQLite integrity check: {}",
                    integrity
                );
            }

            conn.cache_flush()
                .context("Failed to flush SQLite statement cache")?;
            conn.close()
                .map_err(|(_, e)| e)
                .context("Failed to close temporary SQLite database cleanly")?;

            // Atomically replace target database
            progress_cb("Finalizing dictionary...", 0.98);
            std::fs::rename(&tmp_db_path, db_path.as_ref())
                .context("Failed to atomically move new dictionary database into place")?;

            // Verify newly published database can be opened in read-only mode
            let verified = Self::open_read_only_conn(db_path.as_ref())?;
            let verified_count: usize =
                verified.query_row("SELECT COUNT(*) FROM terms", [], |r| r.get(0))?;
            if verified_count == 0 {
                anyhow::bail!("Published dictionary database verification failed: zero entries");
            }

            meta.total_entries = total_inserted;
            progress_cb("Dictionary ready!", 1.0);

            Ok(meta)
        })();

        if import_result.is_err() && tmp_db_path.exists() {
            let _ = std::fs::remove_file(&tmp_db_path);
        }

        import_result
    }
}

fn flush_term_batch(
    conn: &mut Connection,
    batch: &mut Vec<(String, String, String, String, f64, String, i64, String)>,
) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO terms (term, reading, definition_tags, rules, score, glossary, sequence, term_tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for row in batch.drain(..) {
            stmt.execute(params![
                row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn extract_term_bank_number(name: &str) -> u64 {
    if let Some(pos) = name.find("term_bank_") {
        let rest = &name[pos + "term_bank_".len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u64>() {
            return n;
        }
    }
    u64::MAX
}

fn parse_and_validate_term_row(
    item: &serde_json::Value,
    bank_name: &str,
    row_idx: usize,
) -> Result<(String, String, String, String, f64, String, i64, String)> {
    // Yomitan term bank row schema:
    // [expression, reading, definition_tags, rules, score, glossary, sequence?, term_tags?]
    let fields = item.as_array().ok_or_else(|| {
        anyhow::anyhow!("Invalid term row #{row_idx} in {bank_name}: row must be a JSON array")
    })?;
    if fields.len() < 6 {
        anyhow::bail!(
            "Invalid term row #{row_idx} in {bank_name}: expected at least 6 elements, got {}",
            fields.len()
        );
    }

    let term = fields[0]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid term row #{row_idx} in {bank_name}: expression must be a non-empty string"
            )
        })?
        .to_string();

    let reading = fields[1]
        .as_str()
        .ok_or_else(|| {
            anyhow::anyhow!("Invalid term row #{row_idx} in {bank_name}: reading must be a string")
        })?
        .to_string();

    let def_tags = match &fields[2] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        _ => {
            anyhow::bail!(
                "Invalid term row #{row_idx} in {bank_name}: definition_tags must be a string or null"
            );
        }
    };

    let rules = fields[3]
        .as_str()
        .ok_or_else(|| {
            anyhow::anyhow!("Invalid term row #{row_idx} in {bank_name}: rules must be a string")
        })?
        .to_string();

    let score = fields[4]
        .as_f64()
        .filter(|v| v.is_finite())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid term row #{row_idx} in {bank_name}: score must be a finite number"
            )
        })?;

    let gloss_arr = fields[5].as_array().ok_or_else(|| {
        anyhow::anyhow!("Invalid term row #{row_idx} in {bank_name}: glossary must be a JSON array")
    })?;
    if gloss_arr.is_empty() {
        anyhow::bail!(
            "Invalid term row #{row_idx} in {bank_name}: glossary array must not be empty"
        );
    }
    for g_item in gloss_arr {
        let valid = match g_item {
            serde_json::Value::String(s) => !s.trim().is_empty(),
            serde_json::Value::Object(o) => !o.is_empty(),
            serde_json::Value::Array(a) => !a.is_empty(),
            _ => false,
        };
        if !valid {
            anyhow::bail!(
                "Invalid term row #{row_idx} in {bank_name}: glossary contains invalid or empty definition"
            );
        }
    }
    let glossary =
        serde_json::to_string(&fields[5]).context("Failed to serialize validated glossary JSON")?;

    let seq = match fields.get(6) {
        None | Some(serde_json::Value::Null) => 0,
        Some(v) => v.as_i64().ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid term row #{row_idx} in {bank_name}: sequence must be an integer"
            )
        })?,
    };

    let term_tags = match fields.get(7) {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(v) => v
            .as_str()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid term row #{row_idx} in {bank_name}: term_tags must be a string"
                )
            })?
            .to_string(),
    };

    Ok((
        term, reading, def_tags, rules, score, glossary, seq, term_tags,
    ))
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

/// Returns true if the string contains any CJK Kanji character.
pub fn contains_kanji(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '\u{3400}'..='\u{4DBF}'
                | '\u{4E00}'..='\u{9FFF}'
                | '\u{F900}'..='\u{FAFF}'
                | '\u{20000}'..='\u{2EBE0}'
        )
    })
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

    #[test]
    fn test_contains_kanji() {
        assert!(contains_kanji("秋"));
        assert!(contains_kanji("食べる"));
        assert!(contains_kanji("危急存亡の秋"));
        assert!(!contains_kanji("あき"));
        assert!(!contains_kanji("とき"));
        assert!(!contains_kanji("ラーメン"));
        assert!(!contains_kanji("abc 123 !?"));
    }

    fn write_test_zip(path: &Path, entries: &[(&str, &str)]) {
        use std::io::Write;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, content) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn test_import_zip_rejects_invalid_archives_and_preserves_existing_db() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("jitendex.db");

        // 1. Seed a valid 1-row database with "食べる"
        let seed_zip = dir.path().join("seed.zip");
        write_test_zip(
            &seed_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Seeded Jitendex","revision":"2026.01.01","format":3}"#,
                ),
                (
                    "term_bank_1.json",
                    r#"[["食べる","たべる","v1 vt","v1",200.0,["to eat"],100,""]]"#,
                ),
            ],
        );
        let seed_meta = DictDatabase::import_zip(&seed_zip, &db_path, |_, _| {}).unwrap();
        assert_eq!(seed_meta.title, "Seeded Jitendex");
        assert_eq!(seed_meta.total_entries, 1);

        let verify_seeded_intact = |db_path: &Path| {
            assert!(
                !db_path.with_extension("db.tmp").exists(),
                "Temporary database file should be cleaned up on failure"
            );
            let db = DictDatabase::open(db_path).unwrap();
            let meta = db.get_metadata().unwrap();
            assert_eq!(meta.title, "Seeded Jitendex");
            assert_eq!(meta.revision, "2026.01.01");
            assert_eq!(meta.total_entries, 1);
            let cands = vec![DeinflectionCandidate {
                term: "食べる".to_string(),
                rules: 0,
                reasons: vec![],
            }];
            let results = db.find_terms(&cands).unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].term, "食べる");
            assert_eq!(results[0].reading, "たべる");
        };

        // Case (a): Missing index.json
        let missing_index_zip = dir.path().join("missing_index.zip");
        write_test_zip(
            &missing_index_zip,
            &[(
                "term_bank_1.json",
                r#"[["走る","はしる","v5r","v5",100.0,["to run"],1,""]]"#,
            )],
        );
        assert!(DictDatabase::import_zip(&missing_index_zip, &db_path, |_, _| {}).is_err());
        verify_seeded_intact(&db_path);

        // Case (b): term_bank_1.json containing `{}` instead of `[...]`
        let obj_bank_zip = dir.path().join("obj_bank.zip");
        write_test_zip(
            &obj_bank_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Broken Dict","revision":"2026.02.01","format":3}"#,
                ),
                ("term_bank_1.json", r#"{}"#),
            ],
        );
        assert!(DictDatabase::import_zip(&obj_bank_zip, &db_path, |_, _| {}).is_err());
        verify_seeded_intact(&db_path);

        // Case (c): Empty term_bank_1.json `[]` (zero imported entries)
        let empty_bank_zip = dir.path().join("empty_bank.zip");
        write_test_zip(
            &empty_bank_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Empty Dict","revision":"2026.02.01","format":3}"#,
                ),
                ("term_bank_1.json", r#"[]"#),
            ],
        );
        assert!(DictDatabase::import_zip(&empty_bank_zip, &db_path, |_, _| {}).is_err());
        verify_seeded_intact(&db_path);

        // Case (d): Malformed row (empty glossary `[]`)
        let empty_gloss_zip = dir.path().join("empty_gloss.zip");
        write_test_zip(
            &empty_gloss_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Bad Gloss Dict","revision":"2026.02.01","format":3}"#,
                ),
                (
                    "term_bank_1.json",
                    r#"[["走る","はしる","v5r","v5",100.0,[],1,""]]"#,
                ),
            ],
        );
        assert!(DictDatabase::import_zip(&empty_gloss_zip, &db_path, |_, _| {}).is_err());
        verify_seeded_intact(&db_path);

        // Case (e): Malformed row (fewer than 6 elements)
        let short_row_zip = dir.path().join("short_row.zip");
        write_test_zip(
            &short_row_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Short Row Dict","revision":"2026.02.01","format":3}"#,
                ),
                ("term_bank_1.json", r#"[["走る","はしる"]]"#),
            ],
        );
        assert!(DictDatabase::import_zip(&short_row_zip, &db_path, |_, _| {}).is_err());
        verify_seeded_intact(&db_path);

        // Finally: A valid archive replaces the database cleanly
        let valid_zip = dir.path().join("valid_update.zip");
        write_test_zip(
            &valid_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Updated Jitendex","revision":"2026.10.27","format":3}"#,
                ),
                (
                    "term_bank_1.json",
                    r#"[["走る","はしる","v5r","v5",180.0,["to run"],1,""]]"#,
                ),
            ],
        );
        let new_meta = DictDatabase::import_zip(&valid_zip, &db_path, |_, _| {}).unwrap();
        assert_eq!(new_meta.title, "Updated Jitendex");
        assert_eq!(new_meta.total_entries, 1);

        let updated_db = DictDatabase::open(&db_path).unwrap();
        let old_res = updated_db
            .find_terms(&[DeinflectionCandidate {
                term: "食べる".to_string(),
                rules: 0,
                reasons: vec![],
            }])
            .unwrap();
        assert!(old_res.is_empty());
        let new_res = updated_db
            .find_terms(&[DeinflectionCandidate {
                term: "走る".to_string(),
                rules: 0,
                reasons: vec![],
            }])
            .unwrap();
        assert_eq!(new_res.len(), 1);
        assert_eq!(new_res[0].term, "走る");
    }
}
