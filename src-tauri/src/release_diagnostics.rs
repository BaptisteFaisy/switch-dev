use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs,
    path::{Component, Path, PathBuf},
};

/// Identite du frontend monte par le serveur au demarrage.
///
/// Les champs restent optionnels afin qu'une installation incomplete puisse
/// encore exposer `/healthz`. Une release valide doit toutefois tous les
/// renseigner ; le gate compare cette identite avec son manifeste immuable.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReleaseDiagnostics {
    pub(crate) build_id: Option<String>,
    pub(crate) build_commit: Option<String>,
    pub(crate) index_sha256: Option<String>,
    pub(crate) entry_asset: Option<String>,
    pub(crate) entry_asset_sha256: Option<String>,
}

impl ReleaseDiagnostics {
    pub(crate) fn from_static_dir(static_dir: &Path) -> Self {
        let Ok(index_bytes) = fs::read(static_dir.join("index.html")) else {
            return Self::default();
        };
        let index_text = String::from_utf8_lossy(&index_bytes);
        let entry_asset = entry_asset_from_index(&index_text);
        let entry_asset_sha256 = entry_asset
            .as_deref()
            .and_then(safe_relative_asset_path)
            .and_then(|relative| fs::read(static_dir.join(relative)).ok())
            .map(|bytes| sha256_hex(&bytes));

        Self {
            build_id: meta_content(&index_text, "cst-build-id"),
            build_commit: meta_content(&index_text, "cst-build-commit"),
            index_sha256: Some(sha256_hex(&index_bytes)),
            entry_asset,
            entry_asset_sha256,
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02X}");
    }
    output
}

fn meta_content(html: &str, expected_name: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut offset = 0;

    while let Some(relative_start) = lower[offset..].find("<meta") {
        let start = offset + relative_start;
        let end = html[start..].find('>').map(|relative| start + relative + 1)?;
        let tag = &html[start..end];
        if attribute_value(tag, "name")
            .is_some_and(|name| name.eq_ignore_ascii_case(expected_name))
        {
            return attribute_value(tag, "content")
                .map(|content| content.trim().to_string())
                .filter(|content| !content.is_empty());
        }
        offset = end;
    }

    None
}

fn attribute_value(tag: &str, expected_name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let bytes = tag.as_bytes();
    let mut offset = 0;

    while let Some(relative_start) = lower[offset..].find(expected_name) {
        let start = offset + relative_start;
        let end = start + expected_name.len();
        let has_left_boundary = start == 0
            || bytes
                .get(start.wrapping_sub(1))
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'<');
        let has_right_boundary = bytes
            .get(end)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'=');
        if !has_left_boundary || !has_right_boundary {
            offset = end;
            continue;
        }

        let mut cursor = end;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            offset = end;
            continue;
        }
        cursor += 1;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let quote = *bytes.get(cursor)?;
        if quote != b'"' && quote != b'\'' {
            offset = end;
            continue;
        }
        cursor += 1;
        let value_end = bytes[cursor..]
            .iter()
            .position(|byte| *byte == quote)
            .map(|relative| cursor + relative)?;
        return Some(tag[cursor..value_end].to_string());
    }

    None
}

fn entry_asset_from_index(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut assets = BTreeSet::new();
    let mut offset = 0;

    while let Some(relative_start) = lower[offset..].find("assets/index-") {
        let start = offset + relative_start;
        let end = html[start..]
            .find(|character: char| {
                character.is_ascii_whitespace()
                    || matches!(character, '"' | '\'' | ')' | '<' | '>' | '?' | '#')
            })
            .map(|relative| start + relative)
            .unwrap_or(html.len());
        let candidate = html[start..end].replace('\\', "/");
        if candidate.to_ascii_lowercase().ends_with(".js")
            && safe_relative_asset_path(&candidate).is_some()
        {
            assets.insert(candidate);
        }
        offset = end.max(start + 1);
    }

    (assets.len() == 1).then(|| assets.into_iter().next()).flatten()
}

fn safe_relative_asset_path(asset: &str) -> Option<PathBuf> {
    if asset.contains('\\') {
        return None;
    }
    let relative = asset.trim_start_matches('/');
    if relative.is_empty() {
        return None;
    }
    let path = Path::new(relative);
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Some(path.to_path_buf())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    struct TestStaticDir(PathBuf);

    impl TestStaticDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "cst-release-diagnostics-test-{}",
                Uuid::new_v4()
            ));
            fs::create_dir_all(path.join("assets")).expect("create test static directory");
            Self(path)
        }
    }

    impl Drop for TestStaticDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reads_the_mounted_frontend_identity() {
        let static_dir = TestStaticDir::new();
        let entry = b"console.log('mounted release');";
        fs::write(static_dir.0.join("assets/index-AbC123.js"), entry).unwrap();
        fs::write(
            static_dir.0.join("index.html"),
            r#"<!doctype html><html><head>
              <meta content='build-42' name='cst-build-id'>
              <meta name="cst-build-commit" content="abc1234">
              </head><body><script type="module">import("/assets/index-AbC123.js")</script></body></html>"#,
        )
        .unwrap();

        let diagnostics = ReleaseDiagnostics::from_static_dir(&static_dir.0);

        assert_eq!(diagnostics.build_id.as_deref(), Some("build-42"));
        assert_eq!(diagnostics.build_commit.as_deref(), Some("abc1234"));
        assert_eq!(
            diagnostics.entry_asset.as_deref(),
            Some("assets/index-AbC123.js")
        );
        assert_eq!(diagnostics.entry_asset_sha256, Some(sha256_hex(entry)));
        assert_eq!(diagnostics.index_sha256.as_deref().map(str::len), Some(64));
    }

    #[test]
    fn keeps_partial_diagnostics_when_the_entry_asset_is_missing() {
        let static_dir = TestStaticDir::new();
        fs::write(
            static_dir.0.join("index.html"),
            r#"<meta name="cst-build-id" content="build-without-entry">"#,
        )
        .unwrap();

        let diagnostics = ReleaseDiagnostics::from_static_dir(&static_dir.0);

        assert_eq!(
            diagnostics.build_id.as_deref(),
            Some("build-without-entry")
        );
        assert!(diagnostics.index_sha256.is_some());
        assert!(diagnostics.entry_asset.is_none());
        assert!(diagnostics.entry_asset_sha256.is_none());
    }

    #[test]
    fn refuses_ambiguous_or_unsafe_entry_assets() {
        assert_eq!(
            entry_asset_from_index(
                r#"import('/assets/index-one.js'); import('/assets/index-two.js')"#
            ),
            None
        );
        assert_eq!(safe_relative_asset_path("../assets/index-one.js"), None);
        assert_eq!(safe_relative_asset_path("assets\\index-one.js"), None);
    }
}
