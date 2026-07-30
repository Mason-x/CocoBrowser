//! OS-bound protection for small local secrets.

use base64::{engine::general_purpose::STANDARD, Engine as _};

const DPAPI_PREFIX: &str = "dpapi-v1:";
const FALLBACK_PREFIX: &str = "local-v1:";

#[cfg(windows)]
pub fn protect_local(purpose: &str, plaintext: &[u8]) -> Result<String, String> {
  use windows::core::w;
  use windows::Win32::Foundation::{LocalFree, HLOCAL};
  use windows::Win32::Security::Cryptography::{
    CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
  };

  let mut input_bytes = plaintext.to_vec();
  let mut entropy_bytes = purpose.as_bytes().to_vec();
  let input = CRYPT_INTEGER_BLOB {
    cbData: input_bytes
      .len()
      .try_into()
      .map_err(|_| "secret is too large".to_string())?,
    pbData: input_bytes.as_mut_ptr(),
  };
  let entropy = CRYPT_INTEGER_BLOB {
    cbData: entropy_bytes
      .len()
      .try_into()
      .map_err(|_| "secret purpose is too large".to_string())?,
    pbData: entropy_bytes.as_mut_ptr(),
  };
  let mut output = CRYPT_INTEGER_BLOB::default();

  unsafe {
    CryptProtectData(
      &input,
      w!("CocoBrowser local secret"),
      Some(&entropy),
      None,
      None,
      CRYPTPROTECT_UI_FORBIDDEN,
      &mut output,
    )
    .map_err(|e| format!("DPAPI protect failed: {e}"))?;
    let protected = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
    let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
    Ok(format!("{DPAPI_PREFIX}{}", STANDARD.encode(protected)))
  }
}

#[cfg(windows)]
pub fn unprotect_local(purpose: &str, protected: &str) -> Result<Vec<u8>, String> {
  use windows::Win32::Foundation::{LocalFree, HLOCAL};
  use windows::Win32::Security::Cryptography::{
    CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
  };

  let encoded = protected
    .strip_prefix(DPAPI_PREFIX)
    .ok_or_else(|| "unsupported protected-secret format".to_string())?;
  let mut protected_bytes = STANDARD
    .decode(encoded)
    .map_err(|e| format!("invalid protected-secret encoding: {e}"))?;
  let mut entropy_bytes = purpose.as_bytes().to_vec();
  let input = CRYPT_INTEGER_BLOB {
    cbData: protected_bytes
      .len()
      .try_into()
      .map_err(|_| "protected secret is too large".to_string())?,
    pbData: protected_bytes.as_mut_ptr(),
  };
  let entropy = CRYPT_INTEGER_BLOB {
    cbData: entropy_bytes
      .len()
      .try_into()
      .map_err(|_| "secret purpose is too large".to_string())?,
    pbData: entropy_bytes.as_mut_ptr(),
  };
  let mut output = CRYPT_INTEGER_BLOB::default();

  unsafe {
    CryptUnprotectData(
      &input,
      None,
      Some(&entropy),
      None,
      None,
      CRYPTPROTECT_UI_FORBIDDEN,
      &mut output,
    )
    .map_err(|e| format!("DPAPI unprotect failed: {e}"))?;
    let plaintext = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
    let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
    Ok(plaintext)
  }
}

// Non-Windows builds retain an encoded compatibility format. Windows is the
// primary supported security target and uses user-bound DPAPI above.
#[cfg(not(windows))]
pub fn protect_local(_purpose: &str, plaintext: &[u8]) -> Result<String, String> {
  Ok(format!("{FALLBACK_PREFIX}{}", STANDARD.encode(plaintext)))
}

#[cfg(not(windows))]
pub fn unprotect_local(_purpose: &str, protected: &str) -> Result<Vec<u8>, String> {
  let encoded = protected
    .strip_prefix(FALLBACK_PREFIX)
    .ok_or_else(|| "unsupported protected-secret format".to_string())?;
  STANDARD
    .decode(encoded)
    .map_err(|e| format!("invalid protected-secret encoding: {e}"))
}

pub fn is_protected(value: &str) -> bool {
  value.starts_with(DPAPI_PREFIX) || value.starts_with(FALLBACK_PREFIX)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn round_trips_local_secret() {
    let protected = protect_local("test-purpose", b"user:password").unwrap();
    assert!(!protected.contains("password"));
    assert_eq!(
      unprotect_local("test-purpose", &protected).unwrap(),
      b"user:password"
    );
  }
}
