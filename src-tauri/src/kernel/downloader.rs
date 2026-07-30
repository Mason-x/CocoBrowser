//! Verified kernel downloader + atomic installer.
//!
//! Pipeline:
//! 1. Download to `*.partial` with max size from manifest
//! 2. Check Content-Length / streamed size
//! 3. SHA-256 vs manifest
//! 4. ZIP path traversal / absolute path checks
//! 5. Extract to random staging
//! 6. Locate chrome.exe
//! 7. Atomic move to `binaries/<id>/<version>`
//! 8. Write local install registry
//! 10. Failure deletes only staging/partial — never the live install

use super::install_registry::{
  find_executable, install_root, now_secs, InstallRegistryFile, InstalledKernel,
};
use super::manifest::{current_platform_id, KernelAsset, KernelManifest};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use zip::ZipArchive;

#[derive(Debug, thiserror::Error)]
pub enum KernelDownloadError {
  #[error("{0}")]
  Message(String),
  #[error("sha256 mismatch: expected {expected}, got {actual}")]
  Sha256Mismatch { expected: String, actual: String },
  #[error("size mismatch: expected {expected}, got {actual}")]
  SizeMismatch { expected: u64, actual: u64 },
  #[error("unsafe zip entry: {0}")]
  UnsafeZipEntry(String),
  #[error("network: {0}")]
  Network(String),
  #[error("io: {0}")]
  Io(String),
}

impl From<io::Error> for KernelDownloadError {
  fn from(e: io::Error) -> Self {
    KernelDownloadError::Io(e.to_string())
  }
}

/// Compute lowercase hex SHA-256 of bytes (used by install verification).
pub fn sha256_hex(data: &[u8]) -> String {
  let mut hasher = Sha256::new();
  hasher.update(data);
  let digest = hasher.finalize();
  let mut hex = String::with_capacity(digest.len() * 2);
  for byte in digest {
    use std::fmt::Write;
    let _ = write!(hex, "{byte:02x}");
  }
  hex
}

pub fn sha256_file(path: &Path) -> Result<String, KernelDownloadError> {
  let mut file = File::open(path)?;
  let mut hasher = Sha256::new();
  let mut buf = vec![0u8; 1024 * 1024];
  loop {
    let n = file.read(&mut buf)?;
    if n == 0 {
      break;
    }
    hasher.update(&buf[..n]);
  }
  let digest = hasher.finalize();
  let mut hex = String::with_capacity(digest.len() * 2);
  for byte in digest {
    use std::fmt::Write;
    let _ = write!(hex, "{byte:02x}");
  }
  Ok(hex)
}

/// Verify downloaded blob against a manifest asset (size + sha256).
pub fn verify_download_bytes(asset: &KernelAsset, data: &[u8]) -> Result<(), KernelDownloadError> {
  let actual_size = data.len() as u64;
  if actual_size != asset.size {
    return Err(KernelDownloadError::SizeMismatch {
      expected: asset.size,
      actual: actual_size,
    });
  }
  let actual = sha256_hex(data);
  if !actual.eq_ignore_ascii_case(&asset.sha256) {
    return Err(KernelDownloadError::Sha256Mismatch {
      expected: asset.sha256.clone(),
      actual,
    });
  }
  Ok(())
}

pub fn verify_download_file(asset: &KernelAsset, path: &Path) -> Result<(), KernelDownloadError> {
  let meta = fs::metadata(path)?;
  if meta.len() != asset.size {
    return Err(KernelDownloadError::SizeMismatch {
      expected: asset.size,
      actual: meta.len(),
    });
  }
  let actual = sha256_file(path)?;
  if !actual.eq_ignore_ascii_case(&asset.sha256) {
    return Err(KernelDownloadError::Sha256Mismatch {
      expected: asset.sha256.clone(),
      actual,
    });
  }
  Ok(())
}

/// Reject ZIP entry names that escape the extract root.
pub fn is_safe_zip_entry_name(name: &str) -> bool {
  if name.is_empty() {
    return false;
  }
  let path = Path::new(name);
  if path.is_absolute() {
    return false;
  }
  for comp in path.components() {
    match comp {
      std::path::Component::ParentDir => return false,
      std::path::Component::Prefix(_) | std::path::Component::RootDir => return false,
      _ => {}
    }
  }
  // Windows drive / UNC style
  if name.contains(':') {
    return false;
  }
  true
}

/// Extract a ZIP to `dest`, rejecting unsafe entries. No symlink support.
pub fn extract_zip_safe(zip_path: &Path, dest: &Path) -> Result<(), KernelDownloadError> {
  fs::create_dir_all(dest)?;
  let file = File::open(zip_path)?;
  let mut archive =
    ZipArchive::new(file).map_err(|e| KernelDownloadError::Message(format!("open zip: {e}")))?;

  for i in 0..archive.len() {
    let mut entry = archive
      .by_index(i)
      .map_err(|e| KernelDownloadError::Message(format!("zip entry {i}: {e}")))?;
    let name = entry.name().to_string();
    if !is_safe_zip_entry_name(&name) {
      return Err(KernelDownloadError::UnsafeZipEntry(name));
    }
    // Symlinks: zip crate may expose them as files; refuse unix mode symlink bit when present.
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
        return Err(KernelDownloadError::UnsafeZipEntry(format!(
          "symlink refused: {name}"
        )));
      }
    }

    let out_path = dest.join(&name);
    if name.ends_with('/') || entry.is_dir() {
      fs::create_dir_all(&out_path)?;
      continue;
    }
    if let Some(parent) = out_path.parent() {
      fs::create_dir_all(parent)?;
    }
    let mut outfile = File::create(&out_path)?;
    io::copy(&mut entry, &mut outfile)?;
  }
  Ok(())
}

fn cleanup_path(path: &Path) {
  if path.is_dir() {
    let _ = fs::remove_dir_all(path);
  } else if path.exists() {
    let _ = fs::remove_file(path);
  }
}

/// Install from already-verified bytes (tests + local import path).
pub fn install_verified_zip_bytes(
  asset: &KernelAsset,
  data: &[u8],
  binaries_parent: Option<&Path>,
) -> Result<InstalledKernel, KernelDownloadError> {
  verify_download_bytes(asset, data)?;

  let cache = crate::app_dirs::cache_dir().join("kernel_downloads");
  fs::create_dir_all(&cache)?;
  let partial = cache.join(format!(
    "{}-{}-{}.zip.partial",
    asset.id, asset.version, asset.platform
  ));
  {
    let mut f = File::create(&partial)?;
    f.write_all(data)?;
  }

  let result = install_verified_zip_file(asset, &partial, binaries_parent);
  cleanup_path(&partial);
  result
}

/// Install from a verified ZIP file on disk.
pub fn install_verified_zip_file(
  asset: &KernelAsset,
  zip_path: &Path,
  binaries_parent: Option<&Path>,
) -> Result<InstalledKernel, KernelDownloadError> {
  verify_download_file(asset, zip_path)?;

  let staging_parent = crate::app_dirs::cache_dir().join("kernel_staging");
  fs::create_dir_all(&staging_parent)?;
  let staging = staging_parent.join(format!(
    "{}-{}-{}",
    asset.id,
    asset.version,
    uuid::Uuid::new_v4()
  ));
  fs::create_dir_all(&staging)?;

  let install_result = (|| {
    extract_zip_safe(zip_path, &staging)?;
    find_executable(&staging, &asset.executable_candidates).ok_or_else(|| {
      KernelDownloadError::Message(format!(
        "no executable found under staging (candidates: {:?})",
        asset.executable_candidates
      ))
    })?;

    let final_root = if let Some(parent) = binaries_parent {
      parent.join(&asset.id).join(&asset.version)
    } else {
      install_root(&asset.id, &asset.version)
    };

    if let Some(parent) = final_root.parent() {
      fs::create_dir_all(parent)?;
    }

    let parent = final_root
      .parent()
      .ok_or_else(|| KernelDownloadError::Message("invalid kernel install root".into()))?;
    let nonce = uuid::Uuid::new_v4();
    let incoming = parent.join(format!(".incoming-{nonce}"));
    let backup = parent.join(format!(".backup-{nonce}"));

    // Materialize the validated tree beside the destination first. This keeps
    // the current installation untouched if a cross-directory rename/copy
    // fails and makes the final swap stay on one volume.
    if fs::rename(&staging, &incoming).is_err() {
      if let Err(error) = copy_dir_recursive(&staging, &incoming) {
        cleanup_path(&incoming);
        return Err(error);
      }
      cleanup_path(&staging);
    }
    find_executable(&incoming, &asset.executable_candidates).ok_or_else(|| {
      cleanup_path(&incoming);
      KernelDownloadError::Message("executable missing from incoming install".into())
    })?;

    let had_previous = final_root.exists();
    if had_previous {
      fs::rename(&final_root, &backup).map_err(|e| {
        cleanup_path(&incoming);
        KernelDownloadError::Io(format!("stage previous kernel for rollback: {e}"))
      })?;
    }
    if let Err(error) = fs::rename(&incoming, &final_root) {
      if had_previous {
        let _ = fs::rename(&backup, &final_root);
      }
      cleanup_path(&incoming);
      return Err(KernelDownloadError::Io(format!(
        "activate verified kernel: {error}"
      )));
    }

    let exe_final = find_executable(&final_root, &asset.executable_candidates)
      .ok_or_else(|| KernelDownloadError::Message("executable missing after install".into()))?;

    let entry = InstalledKernel {
      id: asset.id.clone(),
      version: asset.version.clone(),
      platform: asset.platform.clone(),
      install_path: final_root.to_string_lossy().to_string(),
      executable: exe_final.to_string_lossy().to_string(),
      sha256: asset.sha256.clone(),
      source_status: asset.source_status.clone(),
      installed_at: now_secs(),
    };

    let mut reg = InstallRegistryFile::load();
    reg.schema_version = 1;
    reg.upsert(entry.clone());
    if let Err(error) = reg.save() {
      // Registry and filesystem form one commit. Restore the old tree if the
      // registry cannot be updated, otherwise the next launch sees a partially
      // committed install.
      cleanup_path(&final_root);
      if had_previous {
        let _ = fs::rename(&backup, &final_root);
      }
      return Err(KernelDownloadError::Message(error));
    }
    cleanup_path(&backup);

    Ok(entry)
  })();

  if install_result.is_err() {
    cleanup_path(&staging);
  }
  install_result
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), KernelDownloadError> {
  fs::create_dir_all(dst)?;
  for entry in fs::read_dir(src)? {
    let entry = entry?;
    let ty = entry.file_type()?;
    let to = dst.join(entry.file_name());
    if ty.is_dir() {
      copy_dir_recursive(&entry.path(), &to)?;
    } else if ty.is_file() {
      fs::copy(entry.path(), to)?;
    } else {
      return Err(KernelDownloadError::Message(format!(
        "refusing to copy special file: {}",
        entry.path().display()
      )));
    }
  }
  Ok(())
}

/// HTTPS download with size cap from the asset, then verified install.
pub async fn download_and_install_asset(
  asset: &KernelAsset,
) -> Result<InstalledKernel, KernelDownloadError> {
  KernelManifest::validate_asset_integrity_fields(asset).map_err(KernelDownloadError::Message)?;
  if !asset.url.starts_with("https://") {
    return Err(KernelDownloadError::Message(
      "refusing non-HTTPS kernel download".into(),
    ));
  }

  // Idempotent: already installed with same hash.
  if let Some(existing) = InstallRegistryFile::load().find(&asset.id, &asset.version) {
    if existing.sha256.eq_ignore_ascii_case(&asset.sha256)
      && Path::new(&existing.executable).is_file()
    {
      log::info!(
        "Kernel {} {} already installed at {}",
        asset.id,
        asset.version,
        existing.install_path
      );
      return Ok(existing.clone());
    }
  }

  let cache = crate::app_dirs::cache_dir().join("kernel_downloads");
  fs::create_dir_all(&cache).map_err(|e| KernelDownloadError::Io(e.to_string()))?;
  let partial = cache.join(format!(
    "{}-{}-{}.zip.partial",
    asset.id, asset.version, asset.platform
  ));
  if partial.exists() {
    cleanup_path(&partial);
  }

  let client = reqwest::Client::builder()
    .connect_timeout(std::time::Duration::from_secs(30))
    .read_timeout(std::time::Duration::from_secs(120))
    .build()
    .map_err(|e| KernelDownloadError::Network(e.to_string()))?;

  let response = client
    .get(&asset.url)
    .send()
    .await
    .map_err(|e| KernelDownloadError::Network(e.to_string()))?
    .error_for_status()
    .map_err(|e| KernelDownloadError::Network(e.to_string()))?;

  if let Some(len) = response.content_length() {
    if len != asset.size {
      return Err(KernelDownloadError::SizeMismatch {
        expected: asset.size,
        actual: len,
      });
    }
  }

  let mut file = File::create(&partial).map_err(|e| KernelDownloadError::Io(e.to_string()))?;
  let mut stream = response.bytes_stream();
  let mut written: u64 = 0;
  use futures_util::StreamExt;
  while let Some(chunk) = stream.next().await {
    let chunk = chunk.map_err(|e| KernelDownloadError::Network(e.to_string()))?;
    written = written.saturating_add(chunk.len() as u64);
    if written > asset.size {
      drop(file);
      cleanup_path(&partial);
      return Err(KernelDownloadError::SizeMismatch {
        expected: asset.size,
        actual: written,
      });
    }
    file
      .write_all(&chunk)
      .map_err(|e| KernelDownloadError::Io(e.to_string()))?;
  }
  drop(file);

  let install = install_verified_zip_file(asset, &partial, None);
  cleanup_path(&partial);
  install
}

/// Look up the audited asset for the current platform.
pub fn planned_fingerprint_chromium_asset() -> Result<KernelAsset, KernelDownloadError> {
  let manifest = KernelManifest::embedded().map_err(KernelDownloadError::Message)?;
  let platform = current_platform_id();
  manifest
    .find("fingerprint-chromium", "148.0.7778.215", platform)
    .cloned()
    .ok_or_else(|| {
      KernelDownloadError::Message(format!(
        "no audited fingerprint-chromium 148 asset for platform {platform}"
      ))
    })
}

pub async fn install_fingerprint_chromium_148() -> Result<InstalledKernel, KernelDownloadError> {
  let asset = planned_fingerprint_chromium_asset()?;
  download_and_install_asset(&asset).await
}

// ── Tauri commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_kernel_manifest() -> Result<KernelManifest, String> {
  KernelManifest::embedded()
}

#[tauri::command]
pub fn list_installed_kernels() -> Result<Vec<InstalledKernel>, String> {
  Ok(InstallRegistryFile::load().kernels)
}

#[tauri::command]
pub async fn install_kernel(id: String, version: String) -> Result<InstalledKernel, String> {
  let manifest = KernelManifest::embedded()?;
  let platform = current_platform_id();
  let asset = manifest
    .find(&id, &version, platform)
    .cloned()
    .ok_or_else(|| format!("No audited kernel asset for {id} {version} on {platform}"))?;
  download_and_install_asset(&asset)
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Write;
  use tempfile::TempDir;
  use zip::write::SimpleFileOptions;
  use zip::ZipWriter;

  fn make_asset(size: u64, sha: &str) -> KernelAsset {
    KernelAsset {
      id: "fingerprint-chromium".into(),
      version: "148.0.7778.215".into(),
      platform: "windows-x64".into(),
      url: "https://example.com/k.zip".into(),
      sha256: sha.into(),
      size,
      executable_candidates: vec!["chrome.exe".into()],
      source_status: "binary-source-delayed".into(),
    }
  }

  fn build_zip_with_chrome(path: &Path) -> Vec<u8> {
    let file = File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("chrome.exe", opts).unwrap();
    // Minimal PE-like header (MZ) so future PE checks can pass.
    zip
      .write_all(b"MZ\0\0fake-chrome-binary-for-tests")
      .unwrap();
    zip.finish().unwrap();
    fs::read(path).unwrap()
  }

  #[test]
  fn sha256_and_size_verification() {
    let data = b"hello-kernel";
    let hash = sha256_hex(data);
    let asset = make_asset(data.len() as u64, &hash);
    verify_download_bytes(&asset, data).unwrap();
    assert!(verify_download_bytes(&asset, b"wrong").is_err());
  }

  #[test]
  fn zip_traversal_rejected() {
    assert!(!is_safe_zip_entry_name("../evil.exe"));
    assert!(!is_safe_zip_entry_name("/abs/path"));
    assert!(!is_safe_zip_entry_name("C:\\Windows\\system32\\x"));
    assert!(is_safe_zip_entry_name("chrome.exe"));
    assert!(is_safe_zip_entry_name("Chromium/Application/chrome.exe"));
  }

  #[test]
  fn extract_rejects_traversal_zip() {
    let tmp = TempDir::new().unwrap();
    let zip_path = tmp.path().join("bad.zip");
    {
      let file = File::create(&zip_path).unwrap();
      let mut zip = ZipWriter::new(file);
      let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
      zip.start_file("../escape.exe", opts).unwrap();
      zip.write_all(b"evil").unwrap();
      zip.finish().unwrap();
    }
    let dest = tmp.path().join("out");
    let err = extract_zip_safe(&zip_path, &dest).unwrap_err();
    match err {
      KernelDownloadError::UnsafeZipEntry(_) => {}
      other => panic!("expected UnsafeZipEntry, got {other}"),
    }
  }

  #[test]
  fn install_correct_zip_and_idempotent() {
    let data_tmp = TempDir::new().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(data_tmp.path().to_path_buf());
    let cache_tmp = TempDir::new().unwrap();
    let _cguard = crate::app_dirs::set_test_cache_dir(cache_tmp.path().to_path_buf());

    let zip_path = data_tmp.path().join("k.zip");
    let bytes = build_zip_with_chrome(&zip_path);
    let hash = sha256_hex(&bytes);
    let asset = make_asset(bytes.len() as u64, &hash);

    let binaries = data_tmp.path().join("binaries");
    let first = install_verified_zip_bytes(&asset, &bytes, Some(&binaries)).unwrap();
    assert!(Path::new(&first.executable).is_file());
    assert_eq!(first.version, "148.0.7778.215");

    // Second install replaces but stays healthy (idempotent path).
    let second = install_verified_zip_bytes(&asset, &bytes, Some(&binaries)).unwrap();
    assert!(Path::new(&second.executable).is_file());
    assert_eq!(
      InstallRegistryFile::load()
        .list_for_id("fingerprint-chromium")
        .len(),
      1
    );
  }

  #[test]
  fn wrong_sha_rejects_without_installing() {
    let data_tmp = TempDir::new().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(data_tmp.path().to_path_buf());
    let cache_tmp = TempDir::new().unwrap();
    let _cguard = crate::app_dirs::set_test_cache_dir(cache_tmp.path().to_path_buf());

    let zip_path = data_tmp.path().join("k.zip");
    let bytes = build_zip_with_chrome(&zip_path);
    let asset = make_asset(bytes.len() as u64, &("00".repeat(32)));
    let binaries = data_tmp.path().join("binaries");
    assert!(install_verified_zip_bytes(&asset, &bytes, Some(&binaries)).is_err());
    assert!(!binaries.join("fingerprint-chromium").exists());
  }

  #[test]
  fn failed_install_preserves_previous_version() {
    let data_tmp = TempDir::new().unwrap();
    let _guard = crate::app_dirs::set_test_data_dir(data_tmp.path().to_path_buf());
    let cache_tmp = TempDir::new().unwrap();
    let _cguard = crate::app_dirs::set_test_cache_dir(cache_tmp.path().to_path_buf());

    let zip_path = data_tmp.path().join("k.zip");
    let bytes = build_zip_with_chrome(&zip_path);
    let hash = sha256_hex(&bytes);
    let asset = make_asset(bytes.len() as u64, &hash);
    let binaries = data_tmp.path().join("binaries");
    let ok = install_verified_zip_bytes(&asset, &bytes, Some(&binaries)).unwrap();
    let exe_before = ok.executable.clone();
    assert!(Path::new(&exe_before).is_file());

    // Bad zip should not wipe the existing install of a *different* version path.
    // Same version replace only happens after verify — wrong sha never reaches extract.
    let bad = make_asset(4, &sha256_hex(b"bad!"));
    assert!(install_verified_zip_bytes(&bad, b"bad!", Some(&binaries)).is_err());
    assert!(Path::new(&exe_before).is_file());
  }

  #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
  #[test]
  fn planned_asset_present_on_windows_x64() {
    let asset = planned_fingerprint_chromium_asset().unwrap();
    assert_eq!(asset.version, "148.0.7778.215");
    assert_eq!(asset.size, 189_767_686);
  }
}
