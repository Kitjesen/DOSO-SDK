use crate::{Error, Pose, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// Generate a canonical ULID before sending a command so its identity can be retained.
pub fn new_request_id() -> Result<String> {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::invalid("system time precedes UNIX epoch"))?
        .as_millis();
    if millis >= (1u128 << 48) {
        return Err(Error::invalid("system time exceeds ULID range"));
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes[6..])
        .map_err(|e| Error::invalid(format!("request identity entropy unavailable: {e}")))?;
    let mut value = (millis << 80) | u128::from_be_bytes(bytes);
    let mut encoded = [0u8; 26];
    for ch in encoded.iter_mut().rev() {
        *ch = ALPHABET[(value & 31) as usize];
        value >>= 5;
    }
    Ok(encoded.into_iter().map(char::from).collect())
}
pub(crate) fn text(value: &str, name: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::invalid(format!("{name} cannot be empty")));
    }
    Ok(())
}
impl Pose {
    pub fn validate(&self) -> Result<()> {
        if [self.x, self.y, self.z, self.yaw]
            .iter()
            .all(|v| v.is_finite())
        {
            Ok(())
        } else {
            Err(Error::invalid("pose requires finite x/y/z/yaw"))
        }
    }
}
