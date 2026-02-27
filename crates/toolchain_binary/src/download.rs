use futures_util::StreamExt;
use std::path::Path;

/// Stream a reqwest response body to a file.
pub async fn stream_to_file(
    response: reqwest::Response,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut file = tokio::fs::File::create(dest).await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    Ok(())
}

pub fn extract_tar_gz(archive_path: &Path, dest: &Path) -> Result<(), std::io::Error> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest)?;
    Ok(())
}

pub fn extract_tar_xz(archive_path: &Path, dest: &Path) -> Result<(), std::io::Error> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = xz2::read::XzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest)?;
    Ok(())
}

pub fn extract_zip_file(
    archive_path: &Path,
    file_in_archive: &str,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut entry = archive.by_name(file_in_archive)?;
    let mut out = std::fs::File::create(dest)?;
    std::io::copy(&mut entry, &mut out)?;
    Ok(())
}

#[cfg(unix)]
pub fn set_executable(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}
