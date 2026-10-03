use super::database::{hiragana_to_katakana, katakana_to_hiragana};
use super::models::{DictMetadata, TermFrequency};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

/// SQLite-backed Yomitan frequency dictionary database.
pub struct FreqDatabase {
    pub conn: Connection,
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

        Ok(Self { conn })
    }

    /// Creates or connects to a frequency SQLite database for reading/writing.
    pub fn open_or_create<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open/create frequency database")?;
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
            CREATE TABLE IF NOT EXISTS frequencies (
                term TEXT NOT NULL,
                reading TEXT,
                rank INTEGER NOT NULL,
                display_value TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_freq_term_reading ON frequencies(term, reading);
            CREATE INDEX IF NOT EXISTS idx_freq_term ON frequencies(term);
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
            .query_row("SELECT COUNT(*) FROM frequencies", [], |r| r.get(0))
            .unwrap_or(0);

        Ok(DictMetadata {
            title: map.remove("title").unwrap_or_else(|| "Jiten Frequency".to_string()),
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
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT rank, display_value FROM frequencies
                 WHERE term = ?1 AND (reading = ?2 OR reading IS NULL OR reading = '')
                 ORDER BY (CASE WHEN reading = ?2 THEN 0 ELSE 1 END), rank ASC
                 LIMIT 1",
            )
            .ok()?;

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
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT rank, display_value FROM frequencies
                 WHERE term = ?1
                 ORDER BY rank ASC
                 LIMIT 1",
            )
            .ok()?;

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
        let file = File::open(zip_path.as_ref())
            .with_context(|| format!("Failed to open zip file at {}", zip_path.as_ref().display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .context("Failed to parse zip archive")?;

        let tmp_db_path = db_path.as_ref().with_extension("tmp.db");
        if tmp_db_path.exists() {
            let _ = std::fs::remove_file(&tmp_db_path);
        }

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

        Self::init_schema(&conn)?;

        // 1. Read index.json
        progress_cb("Reading frequency dictionary metadata...", 0.05);
        let mut meta = DictMetadata::default();
        if let Ok(mut index_file) = archive.by_name("index.json") {
            if let Ok(val) = serde_json::from_reader::<_, serde_json::Value>(&mut index_file) {
                let tx = conn.transaction()?;
                {
                    let mut stmt = tx.prepare("INSERT INTO metadata (key, value) VALUES (?1, ?2)")?;
                    if let Some(obj) = val.as_object() {
                        for (k, v) in obj {
                            let v_str = if let Some(s) = v.as_str() {
                                s.to_string()
                            } else {
                                v.to_string()
                            };
                            stmt.execute(params![k, v_str])?;
                        }
                    }
                }
                tx.commit()?;
                meta = serde_json::from_value(val).unwrap_or_default();
            }
        }

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

            if let Some(arr) = bank_entries.as_array() {
                for item in arr {
                    if let Some((term, reading, rank, display_value)) = parse_frequency_entry(item) {
                        insert_batch.push((term, reading, rank, display_value));
                        total_inserted += 1;
                    }
                }
            }

            if insert_batch.len() >= 10000 {
                let tx = conn.transaction()?;
                {
                    let mut stmt = tx.prepare(
                        "INSERT INTO frequencies (term, reading, rank, display_value)
                         VALUES (?1, ?2, ?3, ?4)"
                    )?;
                    for row in insert_batch.drain(..) {
                        stmt.execute(params![row.0, row.1, row.2, row.3])?;
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
                    "INSERT INTO frequencies (term, reading, rank, display_value)
                     VALUES (?1, ?2, ?3, ?4)"
                )?;
                for row in insert_batch.drain(..) {
                    stmt.execute(params![row.0, row.1, row.2, row.3])?;
                }
            }
            tx.commit()?;
        }

        // 4. Build indexes
        progress_cb("Building frequency indexes...", 0.93);
        conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_freq_term_reading ON frequencies(term, reading);
            CREATE INDEX IF NOT EXISTS idx_freq_term ON frequencies(term);
            ",
        )?;

        drop(conn);

        // Atomically replace target database
        progress_cb("Finalizing frequency dictionary...", 0.98);
        if let Some(parent) = db_path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&tmp_db_path, db_path.as_ref())
            .context("Failed to atomically move new frequency database into place")?;

        meta.total_entries = total_inserted;
        progress_cb("Frequency dictionary ready!", 1.0);

        Ok(meta)
    }
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

    let term = arr[0].as_str()?.to_string();
    let mode = arr[1].as_str()?;
    if mode != "freq" {
        return None;
    }

    let meta = &arr[2];
    if let Some(num) = meta.as_i64() {
        return Some((term, None, num, None));
    }

    if let Some(obj) = meta.as_object() {
        if let Some(reading_val) = obj.get("reading").and_then(|r| r.as_str()) {
            let reading = Some(reading_val.to_string());
            if let Some(freq_val) = obj.get("frequency") {
                if let Some(num) = freq_val.as_i64() {
                    return Some((term, reading, num, None));
                }
                if let Some(fobj) = freq_val.as_object() {
                    let rank = fobj.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
                    let display_value = fobj
                        .get("displayValue")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);
                    return Some((term, reading, rank, display_value));
                }
            }
        } else {
            let rank = obj.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
            let display_value = obj
                .get("displayValue")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
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
}
