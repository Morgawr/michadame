//! SQLite persistence for the mining bank.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// Lightweight row data for listing entries (no image blobs).
#[derive(Clone, Debug, PartialEq)]
pub struct BankEntryMeta {
    pub id: i64,
    /// Unix timestamp in milliseconds.
    pub created_at: i64,
    pub term: String,
    pub reading: String,
    pub definition_text: String,
    /// Serialized dictionary entry (see `bank::entry_from_json`).
    pub definition_json: String,
    pub sentence: String,
    /// Character range `[start, end)` of the mined word within `sentence`.
    pub word_range: (usize, usize),
    pub has_screenshot: bool,
    /// Optional single tag (e.g. the game the word was mined from).
    pub tag: Option<String>,
}

/// A fully-prepared entry ready to be inserted.
#[derive(Clone, Debug, Default)]
pub struct NewBankEntry {
    pub created_at: i64,
    pub term: String,
    pub reading: String,
    pub definition_text: String,
    pub definition_json: String,
    pub sentence: String,
    pub word_range: (usize, usize),
    pub source_text: String,
    /// Encoded (JPEG) screenshot, capped to 720p.
    pub screenshot: Option<Vec<u8>>,
    /// Encoded (JPEG) thumbnail for list previews.
    pub thumbnail: Option<Vec<u8>>,
    pub tag: Option<String>,
}

pub struct BankDatabase {
    conn: Connection,
}

impl BankDatabase {
    /// Opens (creating if necessary) the bank database at `path`.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::from_conn(conn)
    }

    /// Opens a transient in-memory database (used by tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS mined_words (
                 id               INTEGER PRIMARY KEY AUTOINCREMENT,
                 created_at       INTEGER NOT NULL,
                 term             TEXT NOT NULL,
                 reading          TEXT NOT NULL,
                 definition_text  TEXT NOT NULL,
                 definition_json  TEXT NOT NULL,
                 sentence         TEXT NOT NULL,
                 word_start       INTEGER NOT NULL,
                 word_end         INTEGER NOT NULL,
                 source_text      TEXT NOT NULL,
                 screenshot       BLOB,
                 thumbnail        BLOB
             );
             CREATE INDEX IF NOT EXISTS idx_mined_words_created
                 ON mined_words(created_at DESC);",
        )?;
        // Migration: databases created before tags were introduced lack the column.
        let has_tag = conn
            .prepare("SELECT 1 FROM pragma_table_info('mined_words') WHERE name = 'tag'")?
            .exists([])?;
        if !has_tag {
            conn.execute_batch("ALTER TABLE mined_words ADD COLUMN tag TEXT;")?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_mined_words_tag ON mined_words(tag);",
        )?;
        Ok(Self { conn })
    }

    /// Inserts an entry and returns its listing metadata.
    pub fn insert(&self, entry: &NewBankEntry) -> Result<BankEntryMeta> {
        self.conn.execute(
            "INSERT INTO mined_words (created_at, term, reading, definition_text, definition_json,
                 sentence, word_start, word_end, source_text, screenshot, thumbnail, tag)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                entry.created_at,
                entry.term,
                entry.reading,
                entry.definition_text,
                entry.definition_json,
                entry.sentence,
                entry.word_range.0 as i64,
                entry.word_range.1 as i64,
                entry.source_text,
                entry.screenshot,
                entry.thumbnail,
                entry.tag,
            ],
        )?;
        Ok(BankEntryMeta {
            id: self.conn.last_insert_rowid(),
            created_at: entry.created_at,
            term: entry.term.clone(),
            reading: entry.reading.clone(),
            definition_text: entry.definition_text.clone(),
            definition_json: entry.definition_json.clone(),
            sentence: entry.sentence.clone(),
            word_range: entry.word_range,
            has_screenshot: entry.screenshot.is_some(),
            tag: entry.tag.clone(),
        })
    }

    /// Lists all entries, most recent first.
    pub fn list_meta(&self) -> Result<Vec<BankEntryMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, term, reading, definition_text, sentence, word_start, word_end,
                    screenshot IS NOT NULL, definition_json, tag
             FROM mined_words ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(BankEntryMeta {
                id: row.get(0)?,
                created_at: row.get(1)?,
                term: row.get(2)?,
                reading: row.get(3)?,
                definition_text: row.get(4)?,
                sentence: row.get(5)?,
                word_range: (row.get::<_, i64>(6)? as usize, row.get::<_, i64>(7)? as usize),
                has_screenshot: row.get(8)?,
                definition_json: row.get(9)?,
                tag: row.get(10)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn thumbnail(&self, id: i64) -> Result<Option<Vec<u8>>> {
        self.blob(id, "thumbnail")
    }

    pub fn screenshot(&self, id: i64) -> Result<Option<Vec<u8>>> {
        self.blob(id, "screenshot")
    }

    fn blob(&self, id: i64, column: &str) -> Result<Option<Vec<u8>>> {
        let sql = format!("SELECT {column} FROM mined_words WHERE id = ?1");
        Ok(self
            .conn
            .query_row(&sql, params![id], |row| row.get::<_, Option<Vec<u8>>>(0))
            .optional()?
            .flatten())
    }

    /// Sets (or, with `None`, removes) the tag of an entry.
    pub fn set_tag(&self, id: i64, tag: Option<&str>) -> Result<()> {
        self.conn
            .execute("UPDATE mined_words SET tag = ?1 WHERE id = ?2", params![tag, id])?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM mined_words WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(term: &str, created_at: i64, shot: bool) -> NewBankEntry {
        NewBankEntry {
            created_at,
            term: term.into(),
            reading: "よみ".into(),
            definition_text: "① meaning".into(),
            definition_json: "[]".into(),
            sentence: format!("{term}を見た。"),
            word_range: (0, term.chars().count()),
            source_text: format!("前の文。{term}を見た。"),
            screenshot: shot.then(|| vec![1, 2, 3]),
            thumbnail: shot.then(|| vec![4, 5]),
            tag: None,
        }
    }

    #[test]
    fn round_trip_orders_newest_first_and_deletes() {
        let db = BankDatabase::open_in_memory().unwrap();
        let a = db.insert(&entry("猫", 1_000, true)).unwrap();
        let b = db.insert(&entry("犬", 2_000, false)).unwrap();

        let list = db.list_meta().unwrap();
        assert_eq!(list.iter().map(|e| e.id).collect::<Vec<_>>(), vec![b.id, a.id]);
        assert!(list[1].has_screenshot);
        assert!(!list[0].has_screenshot);
        assert_eq!(list[1].word_range, (0, 1));

        assert_eq!(db.screenshot(a.id).unwrap(), Some(vec![1, 2, 3]));
        assert_eq!(db.thumbnail(a.id).unwrap(), Some(vec![4, 5]));
        assert_eq!(db.screenshot(b.id).unwrap(), None);

        db.delete(a.id).unwrap();
        let list = db.list_meta().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].term, "犬");
        assert_eq!(db.screenshot(a.id).unwrap(), None);
    }

    #[test]
    fn tags_round_trip_and_can_be_edited() {
        let db = BankDatabase::open_in_memory().unwrap();
        let a = db
            .insert(&NewBankEntry { tag: Some("Final Fantasy 7".into()), ..entry("猫", 1_000, false) })
            .unwrap();
        assert_eq!(a.tag.as_deref(), Some("Final Fantasy 7"));
        let b = db.insert(&entry("犬", 2_000, false)).unwrap();
        assert_eq!(b.tag, None);

        let list = db.list_meta().unwrap();
        assert_eq!(list[1].tag.as_deref(), Some("Final Fantasy 7"));
        assert_eq!(list[0].tag, None);

        db.set_tag(b.id, Some("Dragon Quest")).unwrap();
        db.set_tag(a.id, None).unwrap();
        let list = db.list_meta().unwrap();
        assert_eq!(list[0].tag.as_deref(), Some("Dragon Quest"));
        assert_eq!(list[1].tag, None);
    }

    #[test]
    fn databases_without_tag_column_are_migrated() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE mined_words (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, created_at INTEGER NOT NULL,
                 term TEXT NOT NULL, reading TEXT NOT NULL, definition_text TEXT NOT NULL,
                 definition_json TEXT NOT NULL, sentence TEXT NOT NULL,
                 word_start INTEGER NOT NULL, word_end INTEGER NOT NULL,
                 source_text TEXT NOT NULL, screenshot BLOB, thumbnail BLOB);
             INSERT INTO mined_words (created_at, term, reading, definition_text, definition_json,
                 sentence, word_start, word_end, source_text)
             VALUES (1, '猫', 'ねこ', 'cat', '[]', '猫だ', 0, 1, '猫だ');",
        )
        .unwrap();
        let db = BankDatabase::from_conn(conn).unwrap();
        let list = db.list_meta().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].tag, None);
        db.set_tag(list[0].id, Some("Grandia 2")).unwrap();
        assert_eq!(db.list_meta().unwrap()[0].tag.as_deref(), Some("Grandia 2"));
    }
}
