use std::ffi::OsStr;
use std::path::{Component, Path};

pub fn validate_segment(kind: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("Invalid {kind}: empty value"));
    }
    if value.len() > 255 {
        return Err(format!("Invalid {kind}: value is too long"));
    }
    if value
        .chars()
        .any(|c| c == '/' || c == '\\' || c == '\0' || c.is_control())
    {
        return Err(format!("Invalid {kind}: path separators are not allowed"));
    }

    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) if name == OsStr::new(value) => Ok(()),
        _ => Err(format!(
            "Invalid {kind}: must be a plain file or folder name"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_segments() {
        for value in ["", ".", "..", "../paper", "paper/one", "paper\\one", "a\0b"] {
            assert!(validate_segment("id", value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn accepts_plain_folder_names() {
        for value in ["paper-1", "2024_title", "Paper With Spaces", "论文"] {
            assert!(validate_segment("id", value).is_ok(), "{value:?}");
        }
    }
}
