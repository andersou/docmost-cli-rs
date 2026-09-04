//! Page content helpers: the MCP-compatible `<status>` shorthand, discovery
//! of local files referenced from markdown, and the ProseMirror nodes that
//! embed uploaded attachments.

use std::path::{Path, PathBuf};

use docmost_client::Attachment;
use regex::Regex;
use serde_json::{Value, json};

const STATUS_COLORS: [&str; 6] = ["gray", "blue", "green", "yellow", "red", "purple"];

/// Rewrites `<status color="green">text</status>` into the `<span>` form that
/// the Docmost status node parses from HTML. Docmost's own markdown
/// converter has no status syntax; raw inline HTML passes through it.
pub fn preprocess_status_tags(markdown: &str) -> String {
    let regex =
        Regex::new(r#"(?is)<status(?:\s+color\s*=\s*["']([A-Za-z]+)["'])?\s*>(.*?)</status>"#)
            .expect("valid regex");
    regex
        .replace_all(markdown, |captures: &regex::Captures<'_>| {
            let color = captures
                .get(1)
                .map(|c| c.as_str().to_ascii_lowercase())
                .filter(|c| STATUS_COLORS.contains(&c.as_str()))
                .unwrap_or_else(|| "gray".to_owned());
            let text = captures.get(2).map(|t| t.as_str().trim()).unwrap_or("");
            format!(r#"<span data-type="status" data-color="{color}">{text}</span>"#)
        })
        .into_owned()
}

/// A markdown link or image whose target is a local file path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalReference {
    /// Byte range of the target inside the parentheses.
    pub range: std::ops::Range<usize>,
    pub target: String,
    pub is_image: bool,
}

fn is_remote(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("/api/files/")
        || lower.starts_with("mailto:")
        || lower.starts_with("data:")
        || lower.starts_with("page:")
        || lower.starts_with('#')
        || lower.starts_with("//")
}

/// Finds `[text](path)` and `![alt](path)` whose path is neither a URL nor
/// an existing Docmost file link. Targets may carry an optional title.
pub fn local_references(markdown: &str) -> Vec<LocalReference> {
    let regex = Regex::new(r#"(!?)\[[^\]]*\]\(([^)\s]+)(?:\s+"[^"]*")?\)"#).expect("valid regex");
    regex
        .captures_iter(markdown)
        .filter_map(|captures| {
            let target = captures.get(2)?;
            let text = target.as_str().trim_matches(['<', '>']);
            if text.is_empty() || is_remote(text) {
                return None;
            }
            Some(LocalReference {
                range: target.range(),
                target: text.to_owned(),
                is_image: captures.get(1).is_some_and(|m| !m.as_str().is_empty()),
            })
        })
        .collect()
}

/// Resolves a markdown target against the base directory.
pub fn resolve_local(base: &Path, target: &str) -> PathBuf {
    let decoded: String = percent_encoding::percent_decode_str(target)
        .decode_utf8_lossy()
        .into_owned();
    let path = Path::new(&decoded);
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

/// Replaces the targets of `references` (which must be sorted by range and
/// non-overlapping) with the given URLs.
pub fn replace_targets(markdown: &str, references: &[LocalReference], urls: &[String]) -> String {
    let mut output = String::with_capacity(markdown.len());
    let mut cursor = 0;
    for (reference, url) in references.iter().zip(urls) {
        output.push_str(&markdown[cursor..reference.range.start]);
        output.push_str(url);
        cursor = reference.range.end;
    }
    output.push_str(&markdown[cursor..]);
    output
}

/// Relative URL the editor uses for an uploaded file.
pub fn file_url(attachment: &Attachment) -> String {
    let name = attachment.file_name.as_deref().unwrap_or("file");
    format!("/api/files/{}/{}", attachment.id, name)
}

/// ProseMirror node that embeds an attachment: `image` and `video` for the
/// matching MIME types, the generic `attachment` card otherwise.
pub fn attachment_node(attachment: &Attachment) -> Value {
    let mime = attachment.mime_type.clone().unwrap_or_default();
    let url = file_url(attachment);
    let size = attachment.file_size.map(Value::from).unwrap_or(Value::Null);
    if mime.starts_with("image/") {
        json!({
            "type": "image",
            "attrs": {
                "src": url,
                "alt": attachment.file_name,
                "attachmentId": attachment.id,
                "size": size,
                "align": "center",
                "width": Value::Null,
            }
        })
    } else if mime.starts_with("video/") {
        json!({
            "type": "video",
            "attrs": {
                "src": url,
                "attachmentId": attachment.id,
                "size": size,
                "align": "center",
                "width": Value::Null,
            }
        })
    } else {
        json!({
            "type": "attachment",
            "attrs": {
                "url": url,
                "name": attachment.file_name,
                "mime": mime,
                "size": size,
                "attachmentId": attachment.id,
            }
        })
    }
}

/// Wraps nodes in a ProseMirror document for `format: json` updates.
pub fn document(nodes: Vec<Value>) -> Value {
    json!({"type": "doc", "content": nodes})
}

/// Serialised ProseMirror document holding one paragraph of plain text,
/// which is the shape `comments/create` expects in its `content` string.
pub fn comment_document(text: &str) -> String {
    let paragraph = if text.is_empty() {
        json!({"type": "paragraph"})
    } else {
        json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
    };
    json!({"type": "doc", "content": [paragraph]}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_tags_become_status_spans() {
        let input = "Ready <status color=\"Green\">STATUS: OK</status> and <status>default</status>\n<STATUS color='pink'>x</STATUS>";
        assert_eq!(
            preprocess_status_tags(input),
            "Ready <span data-type=\"status\" data-color=\"green\">STATUS: OK</span> and <span data-type=\"status\" data-color=\"gray\">default</span>\n<span data-type=\"status\" data-color=\"gray\">x</span>"
        );
    }

    #[test]
    fn finds_only_local_targets() {
        let markdown = "![a](./img/one.png) [doc](docs/spec.pdf \"title\") [web](https://x.y/z) ![api](/api/files/1/a.png) [anchor](#top) [mail](mailto:a@b.c) [page](page:abc)";
        let references = local_references(markdown);
        let targets: Vec<(&str, bool)> = references
            .iter()
            .map(|r| (r.target.as_str(), r.is_image))
            .collect();
        assert_eq!(targets, [("./img/one.png", true), ("docs/spec.pdf", false)]);
        let replaced = replace_targets(
            markdown,
            &references,
            &[
                "/api/files/1/one.png".into(),
                "/api/files/2/spec.pdf".into(),
            ],
        );
        assert!(
            replaced
                .starts_with("![a](/api/files/1/one.png) [doc](/api/files/2/spec.pdf \"title\")")
        );
        assert!(replaced.contains("[web](https://x.y/z)"));
    }

    #[test]
    fn resolves_relative_and_absolute_targets() {
        let base = Path::new("/tmp/base");
        assert_eq!(
            resolve_local(base, "img/a%20b.png"),
            PathBuf::from("/tmp/base/img/a b.png")
        );
        assert_eq!(
            resolve_local(base, "/abs/x.png"),
            PathBuf::from("/abs/x.png")
        );
    }

    #[test]
    fn builds_nodes_by_mime_type() {
        let mut attachment = Attachment {
            id: "att".into(),
            file_name: Some("a.png".into()),
            file_size: Some(10),
            mime_type: Some("image/png".into()),
            url: None,
            extra: Default::default(),
        };
        assert_eq!(attachment_node(&attachment)["type"], "image");
        assert_eq!(
            attachment_node(&attachment)["attrs"]["src"],
            "/api/files/att/a.png"
        );
        attachment.mime_type = Some("video/mp4".into());
        assert_eq!(attachment_node(&attachment)["type"], "video");
        attachment.mime_type = Some("application/pdf".into());
        let node = attachment_node(&attachment);
        assert_eq!(node["type"], "attachment");
        assert_eq!(node["attrs"]["mime"], "application/pdf");
        assert_eq!(node["attrs"]["size"], 10);
    }

    #[test]
    fn comment_document_is_a_json_string() {
        let text = comment_document("hello");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["content"][0]["content"][0]["text"], "hello");
        let empty: Value = serde_json::from_str(&comment_document("")).unwrap();
        assert!(empty["content"][0].get("content").is_none());
    }
}
