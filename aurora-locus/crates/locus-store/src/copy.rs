//! `migrate storage --to postgres`: copies every table of a SQLite install
//! into a freshly migrated, empty Postgres database, in one transaction,
//! then sets every sequence above what was copied.

use locus_core::{CoreError, CoreResult};
use sqlx::{Postgres, QueryBuilder, Row};

use crate::{Store, db_err};

/// What was copied, table by table.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CopyReport {
    pub tables: Vec<(String, u64)>,
    /// The value `blob_key_sequence` was set to.
    pub key_seq: i64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ty {
    BigInt,
    Int,
    Bool,
    Bytes,
    Text,
}

fn ty(pg_type: &str) -> CoreResult<Ty> {
    Ok(match pg_type {
        "bigint" => Ty::BigInt,
        "integer" | "smallint" => Ty::Int,
        "boolean" => Ty::Bool,
        "bytea" => Ty::Bytes,
        "text" | "character varying" => Ty::Text,
        other => {
            return Err(CoreError::Storage(format!(
                "no copy rule for the Postgres type {other}"
            )));
        }
    })
}

/// Rows per insert statement.
const BATCH: i64 = 200;

/// Copies `src` (SQLite) into `dst` (Postgres, migrated, with no install
/// in it yet). Nothing is left in `dst` if it fails.
pub async fn sqlite_to_postgres(src: &Store, dst: &Store) -> CoreResult<CopyReport> {
    let (Store::Sqlite(sp), Store::Postgres(pp)) = (src, dst) else {
        return Err(CoreError::Invalid(
            "migrate storage copies a SQLite install into Postgres".into(),
        ));
    };
    if !dst.key_store_empty().await? {
        return Err(CoreError::Invalid(
            "the Postgres database already holds an install; give an empty one".into(),
        ));
    }
    let mut tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
         AND name NOT LIKE '_sqlx%' ORDER BY name",
    )
    .fetch_all(sp)
    .await
    .map_err(db_err)?;
    // The one foreign key: sessions before their refresh tokens.
    if let Some(i) = tables.iter().position(|t| t == "refresh_token") {
        let t = tables.remove(i);
        tables.push(t);
    }
    let mut report = CopyReport::default();
    let mut tx = pp.begin().await.map_err(db_err)?;
    for table in &tables {
        let cols: Vec<(String, String)> = sqlx::query_as(
            "SELECT column_name::text, data_type::text FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = $1 ORDER BY ordinal_position",
        )
        .bind(table)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err)?;
        if cols.is_empty() {
            // SQLite only (the key sequence counter): handled below.
            continue;
        }
        let types: Vec<Ty> = cols.iter().map(|(_, t)| ty(t)).collect::<CoreResult<_>>()?;
        let list = cols
            .iter()
            .map(|(c, _)| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        // Rows a fresh schema seeds (singletons) give way to the copied ones.
        sqlx::query(&format!("DELETE FROM \"{table}\""))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        let mut copied = 0u64;
        let mut after = i64::MIN;
        loop {
            let rows = sqlx::query(&format!(
                "SELECT rowid, {list} FROM \"{table}\" WHERE rowid > {after} ORDER BY rowid LIMIT {BATCH}"
            ))
            .fetch_all(sp)
            .await
            .map_err(db_err)?;
            if rows.is_empty() {
                break;
            }
            let mut qb: QueryBuilder<Postgres> =
                QueryBuilder::new(format!("INSERT INTO \"{table}\" ({list}) "));
            let mut bad: Option<CoreError> = None;
            qb.push_values(rows.iter(), |mut b, row| {
                for (n, t) in types.iter().enumerate() {
                    let i = n + 1;
                    let r: Result<(), sqlx::Error> = match t {
                        Ty::BigInt => row.try_get::<Option<i64>, _>(i).map(|v| {
                            b.push_bind(v);
                        }),
                        Ty::Int => row.try_get::<Option<i64>, _>(i).map(|v| {
                            b.push_bind(v.map(|n| i32::try_from(n).unwrap_or(i32::MAX)));
                        }),
                        Ty::Bool => row.try_get::<Option<i64>, _>(i).map(|v| {
                            b.push_bind(v.map(|n| n != 0));
                        }),
                        Ty::Bytes => row.try_get::<Option<Vec<u8>>, _>(i).map(|v| {
                            b.push_bind(v);
                        }),
                        Ty::Text => row.try_get::<Option<String>, _>(i).map(|v| {
                            b.push_bind(v);
                        }),
                    };
                    if let Err(e) = r {
                        bad.get_or_insert(CoreError::Storage(format!(
                            "{table}.{}: {e}",
                            cols[n].0
                        )));
                    }
                }
            });
            if let Some(e) = bad {
                return Err(e);
            }
            qb.build().execute(&mut *tx).await.map_err(db_err)?;
            copied += rows.len() as u64;
            after = rows
                .last()
                .map(|r| r.try_get::<i64, _>(0))
                .transpose()
                .map_err(db_err)?
                .unwrap_or(i64::MAX);
        }
        // Serial columns: above the copied ids and SQLite's own counter
        // (AUTOINCREMENT never reuses an id, even a deleted row's).
        let serials: Vec<(String, String)> = sqlx::query_as(
            "SELECT column_name::text, pg_get_serial_sequence($1, column_name)::text FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = $1 \
             AND pg_get_serial_sequence($1, column_name) IS NOT NULL",
        )
        .bind(table)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err)?;
        for (col, seq) in serials {
            let max: Option<i64> =
                sqlx::query_scalar(&format!("SELECT MAX(\"{col}\") FROM \"{table}\""))
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db_err)?;
            // (No sqlite_sequence table when nothing uses AUTOINCREMENT.)
            let counter: Option<i64> =
                sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name = ?1")
                    .bind(table)
                    .fetch_optional(sp)
                    .await
                    .ok()
                    .flatten();
            let next = max.unwrap_or(0).max(counter.unwrap_or(0)).max(1);
            sqlx::query("SELECT setval($1, $2)")
                .bind(&seq)
                .bind(next)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }
        report.tables.push((table.clone(), copied));
    }
    // The key sequence: above the SQLite counter and every copied key_seq.
    let counter: i64 = sqlx::query_scalar("SELECT value FROM key_seq_counter WHERE id = 1")
        .fetch_optional(sp)
        .await
        .map_err(db_err)?
        .unwrap_or(0);
    let copied_max: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(k) FROM (SELECT MAX(key_seq) AS k FROM blob UNION ALL SELECT MAX(key_seq) FROM branding_asset) t",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    let key_seq = counter.max(copied_max.unwrap_or(0)).max(1);
    sqlx::query("SELECT setval('blob_key_sequence', $1)")
        .bind(key_seq)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    report.key_seq = key_seq;
    tx.commit().await.map_err(db_err)?;
    Ok(report)
}
