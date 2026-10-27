use super::database::{hiragana_to_katakana, katakana_to_hiragana};
use super::models::{DictMetadata, TermFrequency};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static FREQ_DB_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_freq_db_generation() -> u64 {
    FREQ_DB_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// SQLite-backed Yomitan frequency dictionary database.
pub struct FreqDatabase {
    pub conn: Connection,
    pub generation: u64,
}

impl FreqDatabase {
    /// Opens an existing frequency dictionary SQLite database in read-only mode for lookups.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .context("Failed to open frequency database")?;

        // Fast read pragma
        let _ = conn.execute_batch("PRAGMA query_only = ON; PRAGMA cache_size = -64000;");

        super::lookup::invalidate_scan_cache();

        Ok(Self {
            conn,
            generation: next_freq_db_generation(),
        })
    }

    /// Creates or connects to a frequency SQLite database for reading/writing.
    pub fn open_or_create<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open/create frequency database")?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn,
            generation: next_freq_db_generation(),
        })
    }

    fn init_tables(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT
            );
            CREATE TABLE IF NOT EXISTS frequencies (
                term TEXT NOT NULL,
                reading TEXT,
                rank INTEGER NOT NULL,
                display_value TEXT
            );
            ",
        )?;
        Ok(())
    }

    fn init_indexes(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_freq_term_reading ON frequencies(term, reading);
            CREATE INDEX IF NOT EXISTS idx_freq_term ON frequencies(term);
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
            .query_row("SELECT COUNT(*) FROM frequencies", [], |r| r.get(0))
            .unwrap_or(0);

        Ok(DictMetadata {
            title: map
                .remove("title")
                .unwrap_or_else(|| "Jiten Frequency".to_string()),
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

    /// Looks up frequency for a given term and reading.
    ///
    /// If a reading is given, only rows for that reading (or reading-less rows for the term)
    /// count, with kana normalization. It never falls back to a *different* reading of the
    /// same term: e.g. `方`/`さま` must not inherit the rank of `方`/`ほう`.
    /// Without a reading, any reading of the term is accepted.
    pub fn get_frequency(&self, term: &str, reading: &str) -> Option<TermFrequency> {
        let clean_reading = reading.trim();
        let clean_term = term.trim();

        let kana_variants = |s: &str| -> Vec<String> {
            let mut v = vec![s.to_string()];
            for alt in [katakana_to_hiragana(s), hiragana_to_katakana(s)] {
                if !v.contains(&alt) {
                    v.push(alt);
                }
            }
            v
        };
        let terms = kana_variants(clean_term);

        // 1. Specific reading: exact (term, reading) match only, with kana normalization.
        if !clean_reading.is_empty() {
            let readings = kana_variants(clean_reading);
            for t in &terms {
                for r in &readings {
                    if let Some(freq) = self.query_single_term_reading(t, r) {
                        return Some(freq);
                    }
                }
            }
            return None;
        }

        // 2. No reading given: accept any reading of the term.
        terms.iter().find_map(|t| self.query_term_any_reading(t))
    }

    fn query_single_term_reading(&self, term: &str, reading: &str) -> Option<TermFrequency> {
        let has_kanji = crate::dict::database::contains_kanji(term);
        let query = if has_kanji {
            "SELECT rank, display_value FROM frequencies
             WHERE term = ?1 AND (reading = ?2 OR reading IS NULL OR reading = '')
             ORDER BY (CASE WHEN reading = ?2 THEN 0 ELSE 1 END),
                      (CASE WHEN display_value LIKE '%㋕%' THEN 1 ELSE 0 END),
                      rank ASC
             LIMIT 1"
        } else {
            "SELECT rank, display_value FROM frequencies
             WHERE term = ?1 AND (reading = ?2 OR reading IS NULL OR reading = '')
             ORDER BY (CASE WHEN reading = ?2 THEN 0 ELSE 1 END),
                      (CASE WHEN display_value LIKE '%㋕%' THEN 0 ELSE 1 END),
                      rank ASC
             LIMIT 1"
        };
        let mut stmt = self.conn.prepare_cached(query).ok()?;

        let mut rows = stmt.query(params![term, reading]).ok()?;
        if let Ok(Some(row)) = rows.next() {
            let rank: i64 = row.get(0).ok()?;
            let display_value: Option<String> = row.get(1).ok();
            return Some(TermFrequency {
                dictionary: "Jiten".to_string(),
                rank,
                display_value,
            });
        }
        None
    }

    fn query_term_any_reading(&self, term: &str) -> Option<TermFrequency> {
        let has_kanji = crate::dict::database::contains_kanji(term);
        let query = if has_kanji {
            "SELECT rank, display_value FROM frequencies
             WHERE term = ?1
             ORDER BY (CASE WHEN display_value LIKE '%㋕%' THEN 1 ELSE 0 END),
                      rank ASC
             LIMIT 1"
        } else {
            "SELECT rank, display_value FROM frequencies
             WHERE term = ?1
             ORDER BY (CASE WHEN display_value LIKE '%㋕%' THEN 0 ELSE 1 END),
                      rank ASC
             LIMIT 1"
        };
        let mut stmt = self.conn.prepare_cached(query).ok()?;

        let mut rows = stmt.query(params![term]).ok()?;
        if let Ok(Some(row)) = rows.next() {
            let rank: i64 = row.get(0).ok()?;
            let display_value: Option<String> = row.get(1).ok();
            return Some(TermFrequency {
                dictionary: "Jiten".to_string(),
                rank,
                display_value,
            });
        }
        None
    }

    /// Alias for [`Self::build_from_zip`] that validates and imports a Yomitan frequency archive.
    pub fn import_zip<P1: AsRef<Path>, P2: AsRef<Path>, F>(
        zip_path: P1,
        db_path: P2,
        progress_cb: F,
    ) -> Result<DictMetadata>
    where
        F: FnMut(&str, f32),
    {
        Self::build_from_zip(zip_path, db_path, progress_cb)
    }

    /// Builds a SQLite database from a Yomitan frequency dictionary ZIP archive.
    pub fn build_from_zip<P1: AsRef<Path>, P2: AsRef<Path>, F>(
        zip_path: P1,
        db_path: P2,
        mut progress_cb: F,
    ) -> Result<DictMetadata>
    where
        F: FnMut(&str, f32),
    {
        progress_cb("Opening frequency dictionary archive...", 0.02);
        let file = File::open(zip_path.as_ref()).with_context(|| {
            format!("Failed to open zip file at {}", zip_path.as_ref().display())
        })?;
        let mut archive = zip::ZipArchive::new(file).context("Failed to parse zip archive")?;

        let tmp_db_path = db_path.as_ref().with_extension("tmp.db");
        if tmp_db_path.exists() {
            let _ = std::fs::remove_file(&tmp_db_path);
        }

        let result = (|| -> Result<(DictMetadata, usize)> {
            let mut conn = Connection::open(&tmp_db_path)
                .context("Failed to open temporary frequency database")?;

            // High performance bulk-load pragmas
            conn.execute_batch(
                "
                PRAGMA synchronous = OFF;
                PRAGMA journal_mode = MEMORY;
                PRAGMA temp_store = MEMORY;
                PRAGMA cache_size = -128000;
                ",
            )?;

            Self::init_tables(&conn)?;

            // 1. Read and strictly validate index.json
            progress_cb("Reading frequency dictionary metadata...", 0.05);
            let meta: DictMetadata = {
                let mut index_file = archive
                    .by_name("index.json")
                    .context("Missing index.json in frequency dictionary archive")?;
                let val: serde_json::Value = serde_json::from_reader(&mut index_file)
                    .context("Failed to parse index.json in frequency dictionary archive")?;
                let obj = val
                    .as_object()
                    .context("index.json root must be a JSON object")?;

                let title = obj
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .context("index.json missing non-empty 'title' field")?
                    .to_string();

                if let Some(format_val) = obj.get("format").or_else(|| obj.get("version")) {
                    let format_num = format_val
                        .as_u64()
                        .context("index.json 'format'/'version' must be a positive integer")?;
                    if !(1..=3).contains(&format_num) {
                        anyhow::bail!(
                            "Unsupported Yomitan frequency dictionary format version: {format_num}"
                        );
                    }
                }

                {
                    let tx = conn.transaction()?;
                    {
                        let mut stmt =
                            tx.prepare("INSERT INTO metadata (key, value) VALUES (?1, ?2)")?;
                        for (k, v) in obj {
                            let v_str = if let Some(s) = v.as_str() {
                                s.to_string()
                            } else {
                                v.to_string()
                            };
                            stmt.execute(params![k, v_str])?;
                        }
                    }
                    tx.commit()?;
                }
                let mut meta: DictMetadata = serde_json::from_value(val)
                    .context("Failed to deserialize frequency dictionary metadata")?;
                meta.title = title;
                meta
            };

            // 2. Discover term_meta_bank_*.json files
            let mut meta_bank_names = Vec::new();
            for i in 0..archive.len() {
                let file = archive.by_index(i)?;
                let name = file.name().to_string();
                if name.ends_with(".json") && name.contains("term_meta_bank_") {
                    meta_bank_names.push(name);
                }
            }
            meta_bank_names.sort();

            let total_banks = meta_bank_names.len();
            if total_banks == 0 {
                anyhow::bail!("No term meta bank files found in frequency dictionary archive");
            }

            // 3. Process each meta bank in bulk transactions
            let mut total_inserted = 0;
            let mut insert_batch: Vec<(String, Option<String>, i64, Option<String>)> =
                Vec::with_capacity(10000);

            for (idx, name) in meta_bank_names.iter().enumerate() {
                let progress = 0.05 + 0.85 * (idx as f32 / total_banks as f32);
                progress_cb(
                    &format!("Importing frequency bank {}/{}...", idx + 1, total_banks),
                    progress,
                );

                let mut file = archive.by_name(name)?;
                let bank_entries: serde_json::Value = serde_json::from_reader(&mut file)
                    .with_context(|| format!("Failed to parse JSON in {name}"))?;

                let arr = bank_entries
                    .as_array()
                    .with_context(|| format!("Invalid {name}: root JSON value must be an array"))?;
                if arr.is_empty() {
                    anyhow::bail!("Frequency meta bank {name} is empty");
                }

                for (row_idx, item) in arr.iter().enumerate() {
                    let row_arr = item.as_array().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Invalid frequency row #{row_idx} in {name}: expected JSON array"
                        )
                    })?;
                    if row_arr.len() < 3 {
                        anyhow::bail!(
                            "Invalid frequency row #{row_idx} in {name}: expected at least 3 elements, got {}",
                            row_arr.len()
                        );
                    }
                    let term = row_arr[0]
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Invalid frequency row #{row_idx} in {name}: term must be a non-empty string"
                            )
                        })?;
                    let mode = row_arr[1].as_str().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Invalid frequency row #{row_idx} in {name}: mode must be a string"
                        )
                    })?;

                    if mode == "freq" {
                        let (_, reading, rank, display_value) =
                            parse_frequency_entry(item).ok_or_else(|| {
                                anyhow::anyhow!(
                                    "Invalid frequency row #{row_idx} in {name}: malformed frequency payload for term '{term}'"
                                )
                            })?;
                        insert_batch.push((term.to_string(), reading, rank, display_value));
                        total_inserted += 1;

                        if insert_batch.len() >= 10000 {
                            let tx = conn.transaction()?;
                            {
                                let mut stmt = tx.prepare(
                                    "INSERT INTO frequencies (term, reading, rank, display_value)
                                     VALUES (?1, ?2, ?3, ?4)",
                                )?;
                                for row in insert_batch.drain(..) {
                                    stmt.execute(params![row.0, row.1, row.2, row.3])?;
                                }
                            }
                            tx.commit()?;
                        }
                    }
                }
            }

            // Flush remainder
            if !insert_batch.is_empty() {
                let tx = conn.transaction()?;
                {
                    let mut stmt = tx.prepare(
                        "INSERT INTO frequencies (term, reading, rank, display_value)
                         VALUES (?1, ?2, ?3, ?4)",
                    )?;
                    for row in insert_batch.drain(..) {
                        stmt.execute(params![row.0, row.1, row.2, row.3])?;
                    }
                }
                tx.commit()?;
            }

            if total_inserted == 0 {
                anyhow::bail!(
                    "Frequency dictionary archive contained zero valid frequency entries"
                );
            }

            // 4. Build indexes
            progress_cb("Building frequency indexes...", 0.93);
            Self::init_indexes(&conn)?;

            // 5. Structural verification before replacing live DB
            let row_count: usize = conn
                .query_row("SELECT COUNT(*) FROM frequencies", [], |r| r.get(0))
                .context("Failed to verify imported frequency row count")?;
            if row_count == 0 || row_count != total_inserted {
                anyhow::bail!(
                    "Imported frequency count mismatch: expected {total_inserted}, found {row_count}"
                );
            }

            let integrity: String = conn
                .query_row("PRAGMA quick_check", [], |r| r.get(0))
                .context("Failed to run PRAGMA quick_check on temporary frequency database")?;
            if !integrity.eq_ignore_ascii_case("ok") {
                anyhow::bail!("Temporary frequency database failed integrity check: {integrity}");
            }

            conn.cache_flush()?;
            conn.close()
                .map_err(|(_, e)| e)
                .context("Failed to close temporary frequency database cleanly")?;

            Ok((meta, total_inserted))
        })();

        let (mut meta, total_inserted) = match result {
            Ok(v) => v,
            Err(err) => {
                let _ = std::fs::remove_file(&tmp_db_path);
                return Err(err);
            }
        };

        // Atomically replace target database
        progress_cb("Finalizing frequency dictionary...", 0.98);
        if let Some(parent) = db_path.as_ref().parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                let _ = std::fs::remove_file(&tmp_db_path);
                return Err(e.into());
            }
        }
        if let Err(e) = std::fs::rename(&tmp_db_path, db_path.as_ref()) {
            let _ = std::fs::remove_file(&tmp_db_path);
            return Err(e).context("Failed to atomically move new frequency database into place");
        }

        // Verify the newly placed frequency database can be opened and queried
        let verified_db = Self::open(db_path.as_ref())
            .context("Failed to reopen newly built frequency database for verification")?;
        let verified_meta = verified_db
            .get_metadata()
            .context("Failed to read metadata from newly built frequency database")?;
        if verified_meta.total_entries == 0 {
            anyhow::bail!("Newly built frequency database reported zero entries after replacement");
        }

        meta.total_entries = total_inserted;
        progress_cb("Frequency dictionary ready!", 1.0);

        Ok(meta)
    }
}

fn parse_freq_value_object(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Option<(i64, Option<String>)> {
    let rank_opt = match obj.get("value") {
        Some(v) => Some(v.as_i64()?),
        None => None,
    };
    let display_value = match obj.get("displayValue") {
        Some(serde_json::Value::Null) | None => None,
        Some(v) => {
            let s = v.as_str()?.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        }
    };
    if rank_opt.is_none() && display_value.is_none() {
        return None;
    }
    Some((rank_opt.unwrap_or(0), display_value))
}

/// Parses an entry from Yomitan frequency meta bank.
/// Supported patterns:
/// - [term, "freq", { "value": rank, "displayValue": "..." }]
/// - [term, "freq", { "reading": "...", "frequency": { "value": rank, "displayValue": "..." } }]
/// - [term, "freq", { "reading": "...", "frequency": rank }]
/// - [term, "freq", rank]
fn parse_frequency_entry(
    item: &serde_json::Value,
) -> Option<(String, Option<String>, i64, Option<String>)> {
    let arr = item.as_array()?;
    if arr.len() < 3 {
        return None;
    }

    let term = arr[0].as_str()?.trim();
    if term.is_empty() {
        return None;
    }
    let term = term.to_string();
    let mode = arr[1].as_str()?;
    if mode != "freq" {
        return None;
    }

    let meta = &arr[2];
    if let Some(num) = meta.as_i64() {
        return Some((term, None, num, None));
    }

    if let Some(obj) = meta.as_object() {
        if let Some(reading_val) = obj.get("reading") {
            let reading_str = reading_val.as_str()?;
            let reading = Some(reading_str.to_string());
            let freq_val = obj.get("frequency")?;
            if let Some(num) = freq_val.as_i64() {
                return Some((term, reading, num, None));
            }
            if let Some(fobj) = freq_val.as_object() {
                let (rank, display_value) = parse_freq_value_object(fobj)?;
                return Some((term, reading, rank, display_value));
            }
            return None;
        } else {
            let (rank, display_value) = parse_freq_value_object(obj)?;
            return Some((term, None, rank, display_value));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frequency_crud_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test_freq.db");

        let db = FreqDatabase::open_or_create(&db_path).unwrap();
        db.conn
            .execute_batch(
                "
                INSERT INTO frequencies (term, reading, rank, display_value) VALUES
                ('私', 'わたし', 23, '23㋕'),
                ('私', 'わたくし', 850, '850㋕'),
                ('する', NULL, 10, '10㋕'),
                ('猫', 'ねこ', 1200, '1200㋕');
                ",
            )
            .unwrap();

        // 1. Exact reading match
        let f_watashi = db.get_frequency("私", "わたし").unwrap();
        assert_eq!(f_watashi.rank, 23);
        assert_eq!(f_watashi.display_value.as_deref(), Some("23㋕"));
        assert_eq!(f_watashi.display_text(), "Jiten: 23㋕");

        let f_watakushi = db.get_frequency("私", "わたくし").unwrap();
        assert_eq!(f_watakushi.rank, 850);

        // 2. Reading with kana normalization (Katakana -> Hiragana)
        let f_watashi_kata = db.get_frequency("私", "ワタシ").unwrap();
        assert_eq!(f_watashi_kata.rank, 23);

        // 3. A different reading of the same term must NOT inherit its rank
        //    (e.g. 方/さま must not get 方/ほう's rank).
        assert!(db.get_frequency("私", "あたし").is_none());

        // 4. Term without reading
        let f_suru = db.get_frequency("する", "").unwrap();
        assert_eq!(f_suru.rank, 10);
        assert_eq!(f_suru.display_value.as_deref(), Some("10㋕"));

        // 5. Unranked word
        assert!(db.get_frequency("存在しない単語", "").is_none());
    }

    #[test]
    fn test_parse_frequency_entry_variants() {
        let entry1 = serde_json::json!(["の", "freq", { "value": 1, "displayValue": "1㋕" }]);
        let (term, reading, rank, dv) = parse_frequency_entry(&entry1).unwrap();
        assert_eq!(term, "の");
        assert_eq!(reading, None);
        assert_eq!(rank, 1);
        assert_eq!(dv.as_deref(), Some("1㋕"));

        let entry2 = serde_json::json!([
            "乃",
            "freq",
            { "reading": "の", "frequency": { "value": 1, "displayValue": "1㋕" } }
        ]);
        let (term, reading, rank, dv) = parse_frequency_entry(&entry2).unwrap();
        assert_eq!(term, "乃");
        assert_eq!(reading.as_deref(), Some("の"));
        assert_eq!(rank, 1);
        assert_eq!(dv.as_deref(), Some("1㋕"));

        let entry3 = serde_json::json!(["猫", "freq", 500]);
        let (term, reading, rank, dv) = parse_frequency_entry(&entry3).unwrap();
        assert_eq!(term, "猫");
        assert_eq!(reading, None);
        assert_eq!(rank, 500);
        assert_eq!(dv, None);
    }

    #[test]
    fn test_build_frequency_db_from_zip() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("test_freq.zip");
        let db_path = dir.path().join("test_freq.db");

        // Create mock frequency zip file
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            zip.start_file("index.json", options).unwrap();
            let index_json = serde_json::json!({
                "title": "Jiten",
                "format": 3,
                "revision": "Jiten 26-10-01",
                "isUpdatable": true,
                "frequencyMode": "rank-based",
                "author": "Jiten"
            });
            zip.write_all(index_json.to_string().as_bytes()).unwrap();

            zip.start_file("term_meta_bank_1.json", options).unwrap();
            let bank_json = serde_json::json!([
                ["私", "freq", { "reading": "わたし", "frequency": { "value": 23, "displayValue": "23㋕" } }],
                ["食べる", "freq", { "value": 150, "displayValue": "150㋕" }]
            ]);
            zip.write_all(bank_json.to_string().as_bytes()).unwrap();

            zip.finish().unwrap();
        }

        // Build database from zip
        let mut progress_calls = 0;
        let meta = FreqDatabase::build_from_zip(&zip_path, &db_path, |_msg, prog| {
            progress_calls += 1;
            assert!(prog >= 0.0 && prog <= 1.0);
        })
        .unwrap();

        assert_eq!(meta.title, "Jiten");
        assert_eq!(meta.revision, "Jiten 26-10-01");
        assert_eq!(meta.total_entries, 2);
        assert!(progress_calls > 0);

        // Open read-only and query entries
        let db = FreqDatabase::open(&db_path).unwrap();
        let f1 = db.get_frequency("私", "わたし").unwrap();
        assert_eq!(f1.rank, 23);
        assert_eq!(f1.display_value.as_deref(), Some("23㋕"));

        let f2 = db.get_frequency("食べる", "").unwrap();
        assert_eq!(f2.rank, 150);
        assert_eq!(f2.display_value.as_deref(), Some("150㋕"));
    }

    #[test]
    fn test_kanji_term_prefers_non_kana_frequency() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("freq_test.db");
        let db = FreqDatabase::open_or_create(&db_path).unwrap();

        db.conn
            .execute_batch(
                "
                INSERT INTO frequencies VALUES ('秋', 'あき', 3170, '3170');
                INSERT INTO frequencies VALUES ('秋', 'あき', 16132, '16132㋕');
                INSERT INTO frequencies VALUES ('秋', 'とき', 368, '368㋕');
                INSERT INTO frequencies VALUES ('秋', 'とき', 280329, '280329');
                INSERT INTO frequencies VALUES ('とき', NULL, 368, '368㋕');
                INSERT INTO frequencies VALUES ('あき', NULL, 16132, '16132㋕');
                ",
            )
            .unwrap();

        // Kanji term 秋 with reading あき must get 3170 (not 16132㋕)
        let f_aki = db.get_frequency("秋", "あき").unwrap();
        assert_eq!(f_aki.rank, 3170);
        assert_eq!(f_aki.display_value.as_deref(), Some("3170"));
        assert!(!f_aki.is_kana_freq());

        // Kanji term 秋 with reading とき must get 280329 (not 368㋕)
        let f_toki = db.get_frequency("秋", "とき").unwrap();
        assert_eq!(f_toki.rank, 280329);
        assert_eq!(f_toki.display_value.as_deref(), Some("280329"));
        assert!(!f_toki.is_kana_freq());

        // Kana term とき must get 368㋕
        let f_kana_toki = db.get_frequency("とき", "").unwrap();
        assert_eq!(f_kana_toki.rank, 368);
        assert_eq!(f_kana_toki.display_value.as_deref(), Some("368㋕"));
        assert!(f_kana_toki.is_kana_freq());

        // Kana term あき must get 16132㋕
        let f_kana_aki = db.get_frequency("あき", "").unwrap();
        assert_eq!(f_kana_aki.rank, 16132);
        assert_eq!(f_kana_aki.display_value.as_deref(), Some("16132㋕"));
        assert!(f_kana_aki.is_kana_freq());
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
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("jiten_freq.db");

        // 1. Seed a valid 1-row frequency database
        let seed_zip = dir.path().join("seed_freq.zip");
        write_test_zip(
            &seed_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Seeded Jiten","revision":"2026.01.01","format":3}"#,
                ),
                (
                    "term_meta_bank_1.json",
                    r#"[["食べる","freq",{"reading":"たべる","frequency":{"value":150,"displayValue":"150㋕"}}]]"#,
                ),
            ],
        );
        let seed_meta = FreqDatabase::import_zip(&seed_zip, &db_path, |_, _| {}).unwrap();
        assert_eq!(seed_meta.title, "Seeded Jiten");
        assert_eq!(seed_meta.total_entries, 1);

        let verify_seeded_intact = |db_path: &Path| {
            assert!(
                !db_path.with_extension("tmp.db").exists(),
                "Temporary frequency database file should be cleaned up on failure"
            );
            let db = FreqDatabase::open(db_path).unwrap();
            let meta = db.get_metadata().unwrap();
            assert_eq!(meta.title, "Seeded Jiten");
            assert_eq!(meta.revision, "2026.01.01");
            assert_eq!(meta.total_entries, 1);
            let f = db.get_frequency("食べる", "たべる").unwrap();
            assert_eq!(f.rank, 150);
            assert_eq!(f.display_value.as_deref(), Some("150㋕"));
        };

        // Case (a): Missing index.json
        let missing_index_zip = dir.path().join("missing_index.zip");
        write_test_zip(
            &missing_index_zip,
            &[("term_meta_bank_1.json", r#"[["走る","freq",42]]"#)],
        );
        assert!(FreqDatabase::import_zip(&missing_index_zip, &db_path, |_, _| {}).is_err());
        verify_seeded_intact(&db_path);

        // Case (b): term_meta_bank_1.json containing `{}` instead of `[...]`
        let obj_bank_zip = dir.path().join("obj_bank.zip");
        write_test_zip(
            &obj_bank_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Broken Jiten","revision":"2026.02.01","format":3}"#,
                ),
                ("term_meta_bank_1.json", r#"{}"#),
            ],
        );
        assert!(FreqDatabase::import_zip(&obj_bank_zip, &db_path, |_, _| {}).is_err());
        verify_seeded_intact(&db_path);

        // Case (c): Empty term_meta_bank_1.json `[]`
        let empty_bank_zip = dir.path().join("empty_bank.zip");
        write_test_zip(
            &empty_bank_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Empty Jiten","revision":"2026.02.01","format":3}"#,
                ),
                ("term_meta_bank_1.json", r#"[]"#),
            ],
        );
        assert!(FreqDatabase::import_zip(&empty_bank_zip, &db_path, |_, _| {}).is_err());
        verify_seeded_intact(&db_path);

        // Case (d): Malformed frequency object `{}` (missing both value and displayValue)
        let bad_payload_zip = dir.path().join("bad_payload.zip");
        write_test_zip(
            &bad_payload_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Bad Payload Jiten","revision":"2026.02.01","format":3}"#,
                ),
                ("term_meta_bank_1.json", r#"[["走る","freq",{}]]"#),
            ],
        );
        assert!(FreqDatabase::import_zip(&bad_payload_zip, &db_path, |_, _| {}).is_err());
        verify_seeded_intact(&db_path);

        // Finally: A valid archive replaces the database cleanly and increments generation
        let db_before = FreqDatabase::open(&db_path).unwrap();
        let gen_before = db_before.generation;
        drop(db_before);

        let valid_zip = dir.path().join("valid_update.zip");
        write_test_zip(
            &valid_zip,
            &[
                (
                    "index.json",
                    r#"{"title":"Updated Jiten","revision":"2026.10.27","format":3}"#,
                ),
                (
                    "term_meta_bank_1.json",
                    r#"[["走る","freq",{"reading":"はしる","frequency":{"value":88,"displayValue":"88㋕"}}]]"#,
                ),
            ],
        );
        let new_meta = FreqDatabase::import_zip(&valid_zip, &db_path, |_, _| {}).unwrap();
        assert_eq!(new_meta.title, "Updated Jiten");
        assert_eq!(new_meta.total_entries, 1);

        let db_after = FreqDatabase::open(&db_path).unwrap();
        assert!(db_after.generation > gen_before);
        assert!(db_after.get_frequency("食べる", "たべる").is_none());
        let f_new = db_after.get_frequency("走る", "はしる").unwrap();
        assert_eq!(f_new.rank, 88);
    }
}
