use anyhow::Result;
use rusqlite::Connection;
use std::path::PathBuf;
use crate::api::auth::get_data_dir;
use log::info;

pub mod images;

pub fn init_db() -> Result<Connection> {
    let mut db_path = get_data_dir().unwrap_or_else(|| PathBuf::from("."));
    db_path.push("spotify_rust.db");

    info!("Inicializando banco de dados SQLite em: {:?}", db_path);
    let conn = Connection::open(db_path)?;

    // Cria as tabelas iniciais
    conn.execute(
        "CREATE TABLE IF NOT EXISTS metadata_cache (
            id TEXT PRIMARY KEY,
            type TEXT NOT NULL,
            json_data TEXT NOT NULL,
            last_updated INTEGER NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS image_cache (
            url TEXT PRIMARY KEY,
            local_path TEXT NOT NULL
        )",
        [],
    )?;

    Ok(conn)
}
