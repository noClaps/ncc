//! Supported compilation targets, independent of runtime environment variables.
pub const NAME: &str = "macos-arm64";
pub const OS: &str = "macos";
pub const ARCH: &str = "arm64";

pub fn validate(name: &str) -> Result<(), String> {
    if name == NAME {
        Ok(())
    } else {
        Err(format!(
            "unsupported target `{name}`; use `ncc --targets` to list supported targets"
        ))
    }
}
