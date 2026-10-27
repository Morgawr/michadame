use super::database::{contains_kanji, hiragana_to_katakana, katakana_to_hiragana};
use super::models::{DictEvent, DictMetadata, PitchAccent, PitchPattern};
use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

static PITCH_DB_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Small combiner kana that attach to the preceding character to form a single mora (yōon /外来語).
/// Note: small `っ`/`ッ` (sokuon) and `ー` (chōonpu) are full morae and are NOT in this set.
fn is_small_combining_kana(c: char) -> bool {
    matches!(
        c,
        'ぁ' | 'ぃ'
            | 'ぅ'
            | 'ぇ'
            | 'ぉ'
            | 'ゃ'
            | 'ゅ'
            | 'ょ'
            | 'ゎ'
            | 'ァ'
            | 'ィ'
            | 'ゥ'
            | 'ェ'
            | 'ォ'
            | 'ャ'
            | 'ュ'
            | 'ョ'
            | 'ヮ'
    )
}

/// Splits a kana reading into individual morae following Yomitan's `getKanaMorae` rules.
pub fn split_kana_morae(text: &str) -> Vec<String> {
    let mut morae: Vec<String> = Vec::new();
    for c in text.chars() {
        if is_small_combining_kana(c) {
            if let Some(last) = morae.last_mut() {
                last.push(c);
                continue;
            }
        }
        morae.push(c.to_string());
    }
    morae
}

/// Determines whether the mora at `mora_index` (0-based; `morae.len()` represents the following
/// particle) has a high pitch for the given `PitchPattern`, matching Yomitan's `isMoraPitchHigh`.
pub fn is_mora_pitch_high(mora_index: usize, pattern: &PitchPattern) -> bool {
    match pattern {
        PitchPattern::Contours(s) => {
            let bytes = s.as_bytes();
            if bytes.is_empty() {
                return false;
            }
            if mora_index < bytes.len() {
                bytes[mora_index] == b'H'
            } else {
                bytes[bytes.len() - 1] == b'H'
            }
        }
        PitchPattern::Downstep(0) => mora_index > 0,
        PitchPattern::Downstep(1) => mora_index < 1,
        PitchPattern::Downstep(pos) => mora_index > 0 && mora_index < (*pos as usize),
    }
}

/// If `c` is a kana character with a dakuten (`゛`), returns the base character without dakuten.
/// Used when rendering nasalized morae (e.g. `が` -> `か` + nasal ring `゜` = `か゚`).
pub fn strip_kana_dakuten(c: char) -> Option<char> {
    match c {
        'ゔ' => Some('う'),
        'が' => Some('か'),
        'ぎ' => Some('き'),
        'ぐ' => Some('く'),
        'げ' => Some('け'),
        'ご' => Some('こ'),
        'ざ' => Some('さ'),
        'じ' => Some('し'),
        'ず' => Some('す'),
        'ぜ' => Some('せ'),
        'ぞ' => Some('そ'),
        'だ' => Some('た'),
        'ぢ' => Some('ち'),
        'づ' => Some('つ'),
        'で' => Some('て'),
        'ど' => Some('と'),
        'ば' => Some('は'),
        'び' => Some('ひ'),
        'ぶ' => Some('ふ'),
        'べ' => Some('へ'),
        'ぼ' => Some('ほ'),
        'ヴ' => Some('ウ'),
        'ガ' => Some('カ'),
        'ギ' => Some('キ'),
        'グ' => Some('ク'),
        'ゲ' => Some('ケ'),
        'ゴ' => Some('コ'),
        'ザ' => Some('サ'),
        'ジ' => Some('シ'),
        'ズ' => Some('ス'),
        'ゼ' => Some('セ'),
        'ゾ' => Some('ソ'),
        'ダ' => Some('タ'),
        'ヂ' => Some('チ'),
        'ヅ' => Some('ツ'),
        'デ' => Some('テ'),
        'ド' => Some('ト'),
        'バ' => Some('ハ'),
        'ビ' => Some('ヒ'),
        'ブ' => Some('フ'),
        'ベ' => Some('ヘ'),
        'ボ' => Some('ホ'),
        'ヷ' => Some('ワ'),
        'ヸ' => Some('ヰ'),
        'ヹ' => Some('ヱ'),
        'ヺ' => Some('ヲ'),
        _ => None,
    }
}

/// SQLite-backed Yomitan pitch accent dictionary database.
pub struct PitchDatabase {
    pub conn: Connection,
    /// Unique generation id per opened database instance (used to invalidate lookup caches when a
    /// new pitch dictionary is imported).
    pub generation: u64,
}

impl PitchDatabase {
    fn from_conn(conn: Connection) -> Self {
        Self {
            conn,
            generation: PITCH_DB_GENERATION.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Opens an existing pitch accent SQLite database in read-only mode for lookups.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .context("Failed to open pitch accent database")?;

        let _ = conn.execute_batch("PRAGMA query_only = ON; PRAGMA cache_size = -32000;");
        Ok(Self::from_conn(conn))
    }

    /// Creates or connects to a pitch accent SQLite database for reading/writing.
    pub fn open_or_create<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open/create pitch accent database")?;
        Self::init_schema(&conn)?;
        Ok(Self::from_conn(conn))
    }

    fn init_tables(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT
            );
            CREATE TABLE IF NOT EXISTS pitches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                term TEXT NOT NULL,
                reading TEXT NOT NULL,
                reading_hira TEXT NOT NULL,
                pitches_json TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    fn init_indexes(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_pitches_term_reading_hira ON pitches(term, reading_hira);
            CREATE INDEX IF NOT EXISTS idx_pitches_term ON pitches(term);
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

        let total_entries: usize = map
            .remove("total_entries")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or_else(|| {
                self.conn
                    .query_row("SELECT COUNT(*) FROM pitches", [], |r| r.get(0))
                    .unwrap_or(0)
            });

        Ok(DictMetadata {
            title: map
                .remove("title")
                .unwrap_or_else(|| "Pitch Dictionary".to_string()),
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

    /// Looks up all valid pitch accent patterns for `(term, reading)`.
    ///
    /// - For kanji terms with a reading, only rows matching `(term, reading)` (normalized to
    ///   Hiragana) are returned; it never falls back to a different reading of the same kanji term.
    /// - For kana-only terms, Hiragana/Katakana variants of `term` are also checked.
    /// - Multiple pitch patterns (whether stored in a single `pitches` array or across multiple
    ///   `term_meta_bank` rows) are merged and deduplicated in insertion order.
    pub fn get_pitches(&self, term: &str, reading: &str) -> Vec<PitchAccent> {
        let clean_term = term.trim();
        let clean_reading = reading.trim();
        if clean_term.is_empty() {
            return Vec::new();
        }

        let has_kanji = contains_kanji(clean_term);
        let mut out: Vec<PitchAccent> = Vec::new();

        if !clean_reading.is_empty() {
            let reading_hira = katakana_to_hiragana(clean_reading);
            if has_kanji {
                self.collect_pitches_for_term_reading(
                    clean_term,
                    clean_reading,
                    &reading_hira,
                    &mut out,
                );
            } else {
                for t in kana_variants(clean_term) {
                    self.collect_pitches_for_term_reading(
                        &t,
                        clean_reading,
                        &reading_hira,
                        &mut out,
                    );
                    if !out.is_empty() {
                        break;
                    }
                }
            }
            return out;
        }

        // Reading is empty: if the term has no kanji, the term itself is the reading.
        if !has_kanji {
            let reading_hira = katakana_to_hiragana(clean_term);
            for t in kana_variants(clean_term) {
                self.collect_pitches_for_term_reading(&t, clean_term, &reading_hira, &mut out);
                if !out.is_empty() {
                    break;
                }
            }
            return out;
        }

        // Kanji term with no reading given: return any pitch entries recorded for this headword.
        self.collect_pitches_any_reading(clean_term, &mut out);
        out
    }

    fn collect_pitches_for_term_reading(
        &self,
        term: &str,
        preferred_reading: &str,
        reading_hira: &str,
        out: &mut Vec<PitchAccent>,
    ) {
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT reading, pitches_json FROM pitches
             WHERE term = ?1 AND (reading_hira = ?2 OR reading = ?3 OR reading = '')
             ORDER BY (CASE WHEN reading = ?3 THEN 0 WHEN reading_hira = ?2 THEN 1 ELSE 2 END), id ASC",
        ) else {
            return;
        };

        let Ok(mut rows) = stmt.query(params![term, reading_hira, preferred_reading]) else {
            return;
        };

        let pref_mora_count = split_kana_morae(preferred_reading).len();
        while let Ok(Some(row)) = rows.next() {
            let row_reading: String = row.get(0).unwrap_or_default();
            let pitches_json: String = row.get(1).unwrap_or_default();
            if let Ok(parsed) = serde_json::from_str::<Vec<PitchAccent>>(&pitches_json) {
                for mut pa in parsed {
                    let chosen_reading = if !preferred_reading.is_empty()
                        && (row_reading.is_empty()
                            || split_kana_morae(&row_reading).len() == pref_mora_count)
                    {
                        preferred_reading.to_string()
                    } else if !row_reading.is_empty() {
                        row_reading.clone()
                    } else {
                        preferred_reading.to_string()
                    };
                    pa.reading = chosen_reading;
                    if !out.iter().any(|existing| {
                        existing.reading == pa.reading
                            && existing.pattern == pa.pattern
                            && existing.nasal == pa.nasal
                            && existing.devoice == pa.devoice
                    }) {
                        out.push(pa);
                    }
                }
            }
        }
    }

    fn collect_pitches_any_reading(&self, term: &str, out: &mut Vec<PitchAccent>) {
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT reading, pitches_json FROM pitches WHERE term = ?1 ORDER BY id ASC",
        ) else {
            return;
        };

        let Ok(mut rows) = stmt.query(params![term]) else {
            return;
        };

        while let Ok(Some(row)) = rows.next() {
            let row_reading: String = row.get(0).unwrap_or_default();
            let pitches_json: String = row.get(1).unwrap_or_default();
            if let Ok(parsed) = serde_json::from_str::<Vec<PitchAccent>>(&pitches_json) {
                for mut pa in parsed {
                    if pa.reading.is_empty() {
                        pa.reading = row_reading.clone();
                    }
                    if !out.iter().any(|existing| {
                        existing.reading == pa.reading
                            && existing.pattern == pa.pattern
                            && existing.nasal == pa.nasal
                            && existing.devoice == pa.devoice
                    }) {
                        out.push(pa);
                    }
                }
            }
        }
    }

    /// Validates and builds a SQLite database from a local Yomitan pitch accent `.zip` archive.
    ///
    /// Validation checks:
    /// 1. The file must be a readable `.zip` archive.
    /// 2. It must contain a valid `index.json` with a dictionary title and supported format/version (1..=3).
    /// 3. It must contain at least one `term_meta_bank_*.json` file.
    /// 4. It must contain at least one valid `"pitch"` entry across the meta banks.
    pub fn build_from_zip<P1: AsRef<Path>, P2: AsRef<Path>, F>(
        zip_path: P1,
        db_path: P2,
        mut progress_cb: F,
    ) -> Result<DictMetadata>
    where
        F: FnMut(&str, f32),
    {
        progress_cb("Opening pitch dictionary archive...", 0.02);
        let file = File::open(zip_path.as_ref()).with_context(|| {
            format!(
                "Failed to open pitch dictionary archive at {}",
                zip_path.as_ref().display()
            )
        })?;
        let mut archive =
            zip::ZipArchive::new(file).context("Selected file is not a valid .zip archive")?;

        // 1. Locate and validate index.json
        progress_cb("Validating dictionary index.json...", 0.05);
        let index_entry_name = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|name| name == "index.json" || name.ends_with("/index.json"))
            .ok_or_else(|| {
                anyhow::anyhow!("Invalid Yomitan dictionary: missing index.json in archive")
            })?;

        let index_val: serde_json::Value = {
            let mut index_file = archive.by_name(&index_entry_name)?;
            read_zip_entry_json(&mut index_file)
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
            })?;

        if let Some(fmt) = index_obj
            .get("format")
            .or_else(|| index_obj.get("version"))
            .and_then(|v| v.as_i64())
        {
            if !(1..=3).contains(&fmt) {
                anyhow::bail!("Unsupported Yomitan dictionary format version: {fmt}");
            }
        }

        // 2. Discover term_meta_bank_*.json files
        let mut meta_bank_names: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            let name = file.name().to_string();
            if name.ends_with(".json") && name.contains("term_meta_bank_") {
                meta_bank_names.push(name);
            }
        }
        meta_bank_names.sort_by(|a, b| {
            extract_bank_number(a)
                .cmp(&extract_bank_number(b))
                .then_with(|| a.cmp(b))
        });

        let total_banks = meta_bank_names.len();
        if total_banks == 0 {
            anyhow::bail!(
                "Invalid pitch accent dictionary '{}': no term_meta_bank_*.json files found",
                title
            );
        }

        // 3. Prepare temporary SQLite database
        if let Some(parent) = db_path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_db_path = db_path.as_ref().with_extension("tmp.db");
        if tmp_db_path.exists() {
            let _ = std::fs::remove_file(&tmp_db_path);
        }

        let import_result = (|| -> Result<DictMetadata> {
            let mut conn = Connection::open(&tmp_db_path)
                .context("Failed to create temporary pitch accent database")?;

            conn.execute_batch(
                "
                PRAGMA synchronous = OFF;
                PRAGMA journal_mode = MEMORY;
                PRAGMA temp_store = MEMORY;
                PRAGMA cache_size = -64000;
                ",
            )?;

            Self::init_tables(&conn)?;

            // Store metadata from index.json
            let mut meta: DictMetadata =
                serde_json::from_value(index_val.clone()).unwrap_or_default();
            {
                let tx = conn.transaction()?;
                {
                    let mut stmt =
                        tx.prepare("INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)")?;
                    for (k, v) in index_obj {
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

            // 4. Parse and insert pitch entries across all meta banks
            let mut total_inserted: usize = 0;
            let mut total_patterns: usize = 0;
            let mut insert_batch: Vec<(String, String, String, String)> =
                Vec::with_capacity(10_000);

            for (idx, name) in meta_bank_names.iter().enumerate() {
                let progress = 0.08 + 0.82 * (idx as f32 / total_banks as f32);
                progress_cb(
                    &format!("Importing pitch bank {}/{}...", idx + 1, total_banks),
                    progress,
                );

                let mut file = archive.by_name(name)?;
                let bank_entries: serde_json::Value = read_zip_entry_json(&mut file)
                    .with_context(|| format!("Failed to parse JSON in {name}"))?;

                let arr = bank_entries.as_array().ok_or_else(|| {
                    anyhow::anyhow!("Invalid term meta bank {name}: root must be a JSON array")
                })?;

                for item in arr {
                    if let Some((term, reading, pitches)) = parse_pitch_entry(item) {
                        if !pitches.is_empty() {
                            total_patterns += pitches.len();
                            let reading_hira = katakana_to_hiragana(&reading);
                            let pitches_json =
                                serde_json::to_string(&pitches).unwrap_or_else(|_| "[]".into());
                            insert_batch.push((term, reading, reading_hira, pitches_json));
                            total_inserted += 1;
                        }
                    }
                }

                if insert_batch.len() >= 10_000 {
                    flush_pitch_batch(&mut conn, &mut insert_batch)?;
                }
            }

            if !insert_batch.is_empty() {
                flush_pitch_batch(&mut conn, &mut insert_batch)?;
            }

            if total_inserted == 0 {
                anyhow::bail!(
                    "Archive '{}' does not contain any supported Yomitan pitch accent ('pitch') entries",
                    meta.title
                );
            }

            // Record total pattern count in metadata
            conn.execute(
                "INSERT OR REPLACE INTO metadata (key, value) VALUES ('total_entries', ?1)",
                params![total_patterns.to_string()],
            )?;

            progress_cb("Building pitch lookup indexes...", 0.93);
            Self::init_indexes(&conn)?;

            drop(conn);

            progress_cb("Finalizing pitch dictionary...", 0.98);
            std::fs::rename(&tmp_db_path, db_path.as_ref()).context(
                "Failed to atomically move imported pitch dictionary database into place",
            )?;

            meta.total_entries = total_patterns;
            progress_cb("Pitch dictionary ready!", 1.0);
            Ok(meta)
        })();

        if import_result.is_err() && tmp_db_path.exists() {
            let _ = std::fs::remove_file(&tmp_db_path);
        }

        import_result
    }
}

fn flush_pitch_batch(
    conn: &mut Connection,
    batch: &mut Vec<(String, String, String, String)>,
) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO pitches (term, reading, reading_hira, pitches_json)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for row in batch.drain(..) {
            stmt.execute(params![row.0, row.1, row.2, row.3])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn extract_bank_number(name: &str) -> u64 {
    if let Some(pos) = name.find("term_meta_bank_") {
        let rest = &name[pos + "term_meta_bank_".len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u64>() {
            return n;
        }
    }
    u64::MAX
}

fn kana_variants(s: &str) -> Vec<String> {
    let mut v = vec![s.to_string()];
    for alt in [katakana_to_hiragana(s), hiragana_to_katakana(s)] {
        if !v.contains(&alt) {
            v.push(alt);
        }
    }
    v
}

/// Reads and parses a JSON file from a ZIP archive entry, tolerating mismatched CRC-32
/// checksums in the ZIP header (some community Yomitan dictionary generators repack JSON
/// entries without updating the stored CRC-32, which browser JSZip ignores) and UTF-8 BOMs.
fn read_zip_entry_json(file: &mut zip::read::ZipFile<'_>) -> Result<serde_json::Value> {
    let mut buf = Vec::with_capacity(file.size() as usize);
    match file.read_to_end(&mut buf) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData && !buf.is_empty() => {
            tracing::warn!(
                "Ignoring ZIP CRC-32 mismatch for '{}' ({} bytes read): {e}",
                file.name(),
                buf.len()
            );
        }
        Err(e) => return Err(e).context("Failed to read entry from zip archive"),
    }
    let slice = buf.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&buf);
    serde_json::from_slice(slice).context("Invalid JSON syntax")
}

/// Parses a single entry from a Yomitan `term_meta_bank_*.json` file if it is a `"pitch"` entry.
///
/// Supported Yomitan v3 and extended formats:
/// - `[term, "pitch", { "reading": "...", "pitches": [ { "position": 0 | "LHH", "nasal": 1 | [1], "devoice": 2 | [2], "tags": [...] }, ... ] }]`
/// - `[term, "pitch", { "reading": "...", "pitches": [ 0, 1 ] }]`
/// - `[term, "pitch", { "reading": "...", "position": 0 }]`
/// - `[term, "pitch", 0]`
pub fn parse_pitch_entry(item: &serde_json::Value) -> Option<(String, String, Vec<PitchAccent>)> {
    let arr = item.as_array()?;
    if arr.len() < 3 {
        return None;
    }

    let term = arr[0].as_str()?.trim().to_string();
    if term.is_empty() {
        return None;
    }
    let mode = arr[1].as_str()?;
    if mode != "pitch" {
        return None;
    }

    let meta = &arr[2];

    // Fallback: [term, "pitch", 0] or [term, "pitch", "LHH"]
    if let Some(pattern) = parse_pitch_pattern(meta) {
        return Some((
            term.clone(),
            term,
            vec![PitchAccent {
                reading: String::new(),
                pattern,
                nasal: Vec::new(),
                devoice: Vec::new(),
                tags: Vec::new(),
            }],
        ));
    }

    let obj = meta.as_object()?;
    let reading = obj
        .get("reading")
        .and_then(|r| r.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&term)
        .to_string();

    let mut pitches = Vec::new();
    if let Some(pitches_arr) = obj.get("pitches").and_then(|p| p.as_array()) {
        for p_item in pitches_arr {
            if let Some(pa) = parse_single_pitch_object(p_item) {
                pitches.push(pa);
            }
        }
    } else if let Some(pa) = parse_single_pitch_object(meta) {
        pitches.push(pa);
    }

    if pitches.is_empty() {
        return None;
    }

    Some((term, reading, pitches))
}

fn parse_single_pitch_object(val: &serde_json::Value) -> Option<PitchAccent> {
    if let Some(pattern) = parse_pitch_pattern(val) {
        return Some(PitchAccent {
            reading: String::new(),
            pattern,
            nasal: Vec::new(),
            devoice: Vec::new(),
            tags: Vec::new(),
        });
    }

    let obj = val.as_object()?;
    let pos_val = obj.get("position")?;
    let pattern = parse_pitch_pattern(pos_val)?;
    let nasal = obj
        .get("nasal")
        .map(parse_mora_positions)
        .unwrap_or_default();
    let devoice = obj
        .get("devoice")
        .map(parse_mora_positions)
        .unwrap_or_default();
    let tags = obj
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::trim))
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();

    Some(PitchAccent {
        reading: String::new(),
        pattern,
        nasal,
        devoice,
        tags,
    })
}

fn parse_pitch_pattern(val: &serde_json::Value) -> Option<PitchPattern> {
    if let Some(n) = val.as_u64() {
        return u32::try_from(n).ok().map(PitchPattern::Downstep);
    }
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Ok(n) = trimmed.parse::<u32>() {
            return Some(PitchPattern::Downstep(n));
        }
        if trimmed.bytes().all(|b| b == b'H' || b == b'L') {
            return Some(PitchPattern::Contours(trimmed.to_string()));
        }
    }
    None
}

fn parse_mora_positions(val: &serde_json::Value) -> Vec<u32> {
    if let Some(n) = val.as_u64() {
        if let Ok(u) = u32::try_from(n) {
            if u > 0 {
                return vec![u];
            }
        }
        return Vec::new();
    }
    if let Some(arr) = val.as_array() {
        let mut out = Vec::new();
        for item in arr {
            if let Some(n) = item.as_u64().and_then(|n| u32::try_from(n).ok()) {
                if n > 0 && !out.contains(&n) {
                    out.push(n);
                }
            }
        }
        return out;
    }
    Vec::new()
}

/// Spawns a background thread to validate and import a local Yomitan pitch accent `.zip` archive.
pub fn spawn_pitch_import(
    zip_path: PathBuf,
    dict_dir: PathBuf,
    is_importing: Arc<AtomicBool>,
    event_tx: Sender<DictEvent>,
) {
    if is_importing.swap(true, Ordering::AcqRel) {
        return;
    }

    std::thread::Builder::new()
        .name("pitch-dict-import-worker".into())
        .spawn(move || {
            let target_db_path = dict_dir.join("pitch.db");
            let tx_clone = event_tx.clone();
            let result =
                PitchDatabase::build_from_zip(&zip_path, &target_db_path, move |msg, progress| {
                    let _ = tx_clone.send(DictEvent::PitchImportProgress {
                        message: msg.to_string(),
                        progress,
                    });
                })
                .map_err(|e| format!("{e:#}"));

            is_importing.store(false, Ordering::Release);
            let _ = event_tx.send(DictEvent::PitchImportFinished(result));
        })
        .expect("Failed to spawn pitch dictionary import thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_kana_morae() {
        assert_eq!(split_kana_morae("たべる"), vec!["た", "べ", "る"]);
        assert_eq!(split_kana_morae("きょうと"), vec!["きょ", "う", "と"]);
        assert_eq!(split_kana_morae("きって"), vec!["き", "っ", "て"]);
        assert_eq!(
            split_kana_morae("パーティー"),
            vec!["パ", "ー", "ティ", "ー"]
        );
        assert_eq!(split_kana_morae("しんぶん"), vec!["し", "ん", "ぶ", "ん"]);
        assert_eq!(split_kana_morae(""), Vec::<String>::new());
    }

    #[test]
    fn test_is_mora_pitch_high() {
        // Heiban [0]: low on mora 0, high on all subsequent morae and following particle
        let heiban = PitchPattern::Downstep(0);
        assert!(!is_mora_pitch_high(0, &heiban));
        assert!(is_mora_pitch_high(1, &heiban));
        assert!(is_mora_pitch_high(2, &heiban));
        assert!(is_mora_pitch_high(3, &heiban));

        // Atamadaka [1]: high on mora 0, low on all subsequent morae
        let atamadaka = PitchPattern::Downstep(1);
        assert!(is_mora_pitch_high(0, &atamadaka));
        assert!(!is_mora_pitch_high(1, &atamadaka));
        assert!(!is_mora_pitch_high(2, &atamadaka));

        // Nakadaka / Odaka [2]: low on mora 0, high on mora 1, drops after mora 1
        let down2 = PitchPattern::Downstep(2);
        assert!(!is_mora_pitch_high(0, &down2));
        assert!(is_mora_pitch_high(1, &down2));
        assert!(!is_mora_pitch_high(2, &down2));

        // Contours string "LHHL"
        let contour = PitchPattern::Contours("LHHL".into());
        assert!(!is_mora_pitch_high(0, &contour));
        assert!(is_mora_pitch_high(1, &contour));
        assert!(is_mora_pitch_high(2, &contour));
        assert!(!is_mora_pitch_high(3, &contour));
        assert!(!is_mora_pitch_high(4, &contour));
    }

    #[test]
    fn test_strip_kana_dakuten() {
        assert_eq!(strip_kana_dakuten('が'), Some('か'));
        assert_eq!(strip_kana_dakuten('ぎ'), Some('き'));
        assert_eq!(strip_kana_dakuten('ぐ'), Some('く'));
        assert_eq!(strip_kana_dakuten('げ'), Some('け'));
        assert_eq!(strip_kana_dakuten('ご'), Some('こ'));
        assert_eq!(strip_kana_dakuten('ガ'), Some('カ'));
        assert_eq!(strip_kana_dakuten('か'), None);
        assert_eq!(strip_kana_dakuten('ん'), None);
    }

    #[test]
    fn test_parse_pitch_entry_variants() {
        // 1. Standard Yomitan v3 pitch entry with multiple pitches, nasal, devoice, and tags
        let v1 = serde_json::json!([
            "食べる",
            "pitch",
            {
                "reading": "たべる",
                "pitches": [
                    { "position": 2, "nasal": [2], "devoice": [3], "tags": ["v1"] },
                    { "position": 1, "nasal": 1, "devoice": 2 }
                ]
            }
        ]);
        let (term, reading, pitches) = parse_pitch_entry(&v1).unwrap();
        assert_eq!(term, "食べる");
        assert_eq!(reading, "たべる");
        assert_eq!(pitches.len(), 2);
        assert_eq!(pitches[0].pattern, PitchPattern::Downstep(2));
        assert_eq!(pitches[0].nasal, vec![2]);
        assert_eq!(pitches[0].devoice, vec![3]);
        assert_eq!(pitches[0].tags, vec!["v1"]);
        assert_eq!(pitches[1].pattern, PitchPattern::Downstep(1));
        assert_eq!(pitches[1].nasal, vec![1]);
        assert_eq!(pitches[1].devoice, vec![2]);

        // 2. H/L string contour position
        let v2 = serde_json::json!([
            "橋",
            "pitch",
            {
                "reading": "はし",
                "pitches": [
                    { "position": "LHL" }
                ]
            }
        ]);
        let (_, _, pitches2) = parse_pitch_entry(&v2).unwrap();
        assert_eq!(pitches2[0].pattern, PitchPattern::Contours("LHL".into()));

        // 3. Non-pitch entry (e.g. "freq" or "ipa") is ignored
        let freq_entry = serde_json::json!(["猫", "freq", 500]);
        assert!(parse_pitch_entry(&freq_entry).is_none());
    }

    #[test]
    fn test_build_and_query_pitch_database_from_zip() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("mock_pitch.zip");
        let db_path = dir.path().join("pitch.db");

        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            zip.start_file("index.json", options).unwrap();
            let index_json = serde_json::json!({
                "title": "Test Accent Dict",
                "format": 3,
                "revision": "1.0",
                "author": "Tester"
            });
            zip.write_all(index_json.to_string().as_bytes()).unwrap();

            zip.start_file("term_meta_bank_1.json", options).unwrap();
            let bank_json = serde_json::json!([
                ["箸", "pitch", { "reading": "はし", "pitches": [{ "position": 1 }] }],
                ["橋", "pitch", { "reading": "はし", "pitches": [{ "position": 2 }] }],
                ["端", "pitch", { "reading": "はし", "pitches": [{ "position": 0 }] }],
                // Word with multiple pitch accents inside one `pitches` array
                ["秋刀魚", "pitch", { "reading": "さんま", "pitches": [{ "position": 0 }, { "position": 3 }] }],
                // Word with multiple pitch accents split across two separate rows
                ["其処", "pitch", { "reading": "そこ", "pitches": [{ "position": 0 }] }],
                ["其処", "pitch", { "reading": "そこ", "pitches": [{ "position": 1 }] }],
                // Word with nasal and devoiced morae
                ["鏡", "pitch", { "reading": "かがみ", "pitches": [{ "position": 3, "nasal": [2], "devoice": [1] }] }],
                // Kana-only word
                ["うんざり", "pitch", { "reading": "うんざり", "pitches": [{ "position": 3 }] }]
            ]);
            zip.write_all(bank_json.to_string().as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        let meta = PitchDatabase::build_from_zip(&zip_path, &db_path, |_, _| {}).unwrap();
        assert_eq!(meta.title, "Test Accent Dict");
        assert_eq!(meta.revision, "1.0");
        assert_eq!(meta.total_entries, 9);

        let db = PitchDatabase::open(&db_path).unwrap();
        let loaded_meta = db.get_metadata().unwrap();
        assert_eq!(loaded_meta.title, "Test Accent Dict");
        assert_eq!(loaded_meta.total_entries, 9);

        // Homophones with different kanji must get their own pitch
        let p_chopsticks = db.get_pitches("箸", "はし");
        assert_eq!(p_chopsticks.len(), 1);
        assert_eq!(p_chopsticks[0].pattern, PitchPattern::Downstep(1));

        let p_bridge = db.get_pitches("橋", "はし");
        assert_eq!(p_bridge.len(), 1);
        assert_eq!(p_bridge[0].pattern, PitchPattern::Downstep(2));

        let p_edge = db.get_pitches("端", "はし");
        assert_eq!(p_edge.len(), 1);
        assert_eq!(p_edge[0].pattern, PitchPattern::Downstep(0));

        // Different reading of a kanji word must NOT inherit another reading's pitch
        assert!(db.get_pitches("橋", "きょう").is_empty());

        // Multiple pitches in single array
        let p_sanma = db.get_pitches("秋刀魚", "さんま");
        assert_eq!(p_sanma.len(), 2);
        assert_eq!(p_sanma[0].pattern, PitchPattern::Downstep(0));
        assert_eq!(p_sanma[1].pattern, PitchPattern::Downstep(3));

        // Multiple pitches across multiple rows
        let p_soko = db.get_pitches("其処", "そこ");
        assert_eq!(p_soko.len(), 2);
        assert_eq!(p_soko[0].pattern, PitchPattern::Downstep(0));
        assert_eq!(p_soko[1].pattern, PitchPattern::Downstep(1));

        // Nasal & devoice preservation
        let p_kagami = db.get_pitches("鏡", "かがみ");
        assert_eq!(p_kagami.len(), 1);
        assert_eq!(p_kagami[0].nasal, vec![2]);
        assert_eq!(p_kagami[0].devoice, vec![1]);

        // Kana-only word queried with empty reading or Katakana variant
        let p_unzari = db.get_pitches("うんざり", "");
        assert_eq!(p_unzari.len(), 1);
        assert_eq!(p_unzari[0].pattern, PitchPattern::Downstep(3));
        assert_eq!(p_unzari[0].reading, "うんざり");
    }

    #[test]
    fn test_rejects_invalid_or_non_pitch_zip_archives() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("pitch.db");

        // 1. Frequency dictionary (has term_meta_bank_1.json, but only "freq" entries, no "pitch")
        let freq_zip = dir.path().join("freq_only.zip");
        {
            let file = File::create(&freq_zip).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("index.json", options).unwrap();
            zip.write_all(r#"{"title":"Freq Dict","format":3,"revision":"1"}"#.as_bytes())
                .unwrap();
            zip.start_file("term_meta_bank_1.json", options).unwrap();
            zip.write_all(r#"[["猫","freq",100]]"#.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        let err = PitchDatabase::build_from_zip(&freq_zip, &db_path, |_, _| {}).unwrap_err();
        assert!(
            err.to_string().contains("pitch"),
            "Expected pitch validation error, got: {err}"
        );
        assert!(!db_path.exists());

        // 2. Archive missing index.json
        let no_index_zip = dir.path().join("no_index.zip");
        {
            let file = File::create(&no_index_zip).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("term_meta_bank_1.json", options).unwrap();
            zip.write_all(
                r#"[["猫","pitch",{"reading":"ねこ","pitches":[{"position":1}]}]]"#.as_bytes(),
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let err2 = PitchDatabase::build_from_zip(&no_index_zip, &db_path, |_, _| {}).unwrap_err();
        assert!(err2.to_string().contains("index.json"));
        assert!(!db_path.exists());
    }

    #[test]
    fn test_build_pitch_database_tolerates_mismatched_zip_crc32() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("bad_crc_pitch.zip");
        let db_path = dir.path().join("pitch.db");

        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("index.json", options).unwrap();
            zip.write_all(r#"{"title":"Repacked Pitch","format":3,"revision":"1.0"}"#.as_bytes())
                .unwrap();
            zip.start_file("term_meta_bank_1.json", options).unwrap();
            zip.write_all(
                r#"[["猫","pitch",{"reading":"ねこ","pitches":[{"position":1}]}]]"#.as_bytes(),
            )
            .unwrap();
            zip.finish().unwrap();
        }

        // Corrupt the stored CRC-32 fields in both local file headers (PK\x03\x04 + 14)
        // and central directory headers (PK\x01\x02 + 16) while keeping deflate streams intact.
        let mut raw = std::fs::read(&zip_path).unwrap();
        let mut i = 0;
        while i + 20 <= raw.len() {
            if &raw[i..i + 4] == b"PK\x03\x04" {
                raw[i + 14] ^= 0xFF;
            } else if &raw[i..i + 4] == b"PK\x01\x02" {
                raw[i + 16] ^= 0xFF;
            }
            i += 1;
        }
        std::fs::write(&zip_path, &raw).unwrap();

        let meta = PitchDatabase::build_from_zip(&zip_path, &db_path, |_, _| {}).unwrap();
        assert_eq!(meta.title, "Repacked Pitch");
        assert_eq!(meta.total_entries, 1);

        let db = PitchDatabase::open(&db_path).unwrap();
        let pitches = db.get_pitches("猫", "ねこ");
        assert_eq!(pitches.len(), 1);
        assert_eq!(pitches[0].pattern, PitchPattern::Downstep(1));
    }
}
