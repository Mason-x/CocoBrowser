//! Per-profile window colour derivation.
//!
//! Each profile gets a stable pastel colour derived from its UUID, so windows
//! belonging to different profiles are easy to tell apart at a glance.

/// Derive a stable pastel hex colour for a profile id.
///
/// FNV-1a over the 16 id bytes gives a hue in [0,360); saturation and lightness
/// are fixed to a pastel band so the window frame stays readable rather than
/// garish. The same id always yields the same colour.
pub fn derive_profile_color(id: &uuid::Uuid) -> String {
  let mut h: u32 = 2166136261;
  for &b in id.as_bytes() {
    h = (h ^ u32::from(b)).wrapping_mul(16777619);
  }
  let hue = f64::from(h % 360);
  let (r, g, b) = hsl_to_rgb(hue, 0.6, 0.8);
  format!("#{r:02x}{g:02x}{b:02x}")
}

/// Convert HSL (h in [0,360), s/l in [0,1]) to 8-bit RGB.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
  let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
  let hp = h / 60.0;
  let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
  let (r1, g1, b1) = match hp as i32 {
    0 => (c, x, 0.0),
    1 => (x, c, 0.0),
    2 => (0.0, c, x),
    3 => (0.0, x, c),
    4 => (x, 0.0, c),
    _ => (c, 0.0, x),
  };
  let m = l - c / 2.0;
  let to_u8 = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
  (to_u8(r1), to_u8(g1), to_u8(b1))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn colour_is_stable_for_the_same_id() {
    let id = uuid::Uuid::parse_str("0a21f217-e7b4-41a2-bc89-57b26d6045f4").unwrap();
    assert_eq!(derive_profile_color(&id), derive_profile_color(&id));
  }

  #[test]
  fn different_ids_generally_differ() {
    let a = derive_profile_color(&uuid::Uuid::new_v4());
    let b = derive_profile_color(&uuid::Uuid::new_v4());
    // Hue space is 360 wide, so a collision is possible but must not be the norm.
    let c = derive_profile_color(&uuid::Uuid::new_v4());
    assert!(a != b || b != c, "three random ids all produced one colour");
  }

  #[test]
  fn output_is_a_six_digit_hex_colour() {
    let colour = derive_profile_color(&uuid::Uuid::new_v4());
    assert_eq!(colour.len(), 7);
    assert!(colour.starts_with('#'));
    assert!(colour[1..].chars().all(|c| c.is_ascii_hexdigit()));
  }
}
