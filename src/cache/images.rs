use anyhow::Result;
use log::{info, warn};
use rusqlite::Connection;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// Faz o download da imagem e salva localmente, registrando o caminho no SQLite
#[allow(dead_code)]
pub async fn fetch_and_cache_image(
    db: &Connection,
    url: &str,
    data_dir: &PathBuf,
) -> Result<PathBuf> {
    // Verifica se já existe no banco
    let mut stmt = db.prepare("SELECT local_path FROM image_cache WHERE url = ?1")?;
    if let Ok(local_path) = stmt.query_row([url], |row| row.get::<_, String>(0)) {
        let p = PathBuf::from(&local_path);
        if p.exists() {
            return Ok(p);
        } else {
            // Se o arquivo foi deletado por fora, limpa do banco e baixa de novo
            warn!("Imagem estava no banco, mas arquivo sumiu do disco. Baixando novamente...");
            db.execute("DELETE FROM image_cache WHERE url = ?1", [url])?;
        }
    }

    info!("Baixando nova capa de álbum: {}", url);
    let client = reqwest::Client::new();
    let response = client.get(url).send().await?;
    let bytes = response.bytes().await?;

    // Criar um nome de arquivo único baseado num hash básico da URL
    let hash = format!("{:x}", md5::compute(url));
    let mut file_path = data_dir.clone();
    file_path.push("covers");
    
    if !file_path.exists() {
        fs::create_dir_all(&file_path)?;
    }
    
    file_path.push(format!("{}.jpg", hash));

    // Salvar no disco
    let mut file = fs::File::create(&file_path)?;
    file.write_all(&bytes)?;

    // Registrar no banco
    let path_str = file_path.to_string_lossy().into_owned();
    db.execute(
        "INSERT INTO image_cache (url, local_path) VALUES (?1, ?2)",
        (url, &path_str),
    )?;

    Ok(file_path)
}
