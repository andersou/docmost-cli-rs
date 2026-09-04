//! Page links in the exact shape the Docmost web client builds them:
//! `<app>/s/<space slug>/p/<title slug>-<slugId>`. Only the trailing slugId
//! is used by the client to resolve the page; the title part is cosmetic.

const MAX_TITLE_CHARS: usize = 70;

/// Mirrors `@sindresorhus/slugify` with its defaults: transliterate to
/// ASCII, split camelCase words (`InfluxDB` -> `influx-db`), lowercase, and
/// collapse every non-alphanumeric run into a single hyphen.
pub fn slugify(title: &str) -> String {
    // Symbols and emoji are dropped rather than spelled out, matching the
    // web client; letters keep their transliteration (ção -> cao).
    let letters: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_ascii())
        .collect();
    let ascii = decamelize(&deunicode::deunicode(&letters)).to_ascii_lowercase();
    let mut slug = String::with_capacity(ascii.len());
    for c in ascii.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_owned()
}

/// Port of the `decamelize` package used by slugify: a separator goes
/// between a lowercase letter or digit and an uppercase letter, and between
/// an uppercase letter and an uppercase-led word (`myURLstring` ->
/// `my-ur-lstring`).
fn decamelize(text: &str) -> String {
    let lower_upper = regex::Regex::new(r"([\p{Ll}\d])(\p{Lu})").expect("valid regex");
    let upper_word = regex::Regex::new(r"(\p{Lu})(\p{Lu}\p{Ll}+)").expect("valid regex");
    let split = lower_upper.replace_all(text, "$1-$2");
    upper_word.replace_all(&split, "$1-$2").into_owned()
}

/// URL of a page inside its space.
pub fn page_url(app_url: &str, space_slug: &str, slug_id: &str, title: Option<&str>) -> String {
    let title: String = title
        .unwrap_or_default()
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect();
    let mut slug = slugify(&title);
    if slug.is_empty() {
        slug = "untitled".into();
    }
    format!(
        "{}/s/{}/p/{}-{}",
        app_url.trim_end_matches('/'),
        space_slug,
        slug,
        slug_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugifies_like_the_web_client() {
        assert_eq!(slugify("docmost-cli smoke test"), "docmost-cli-smoke-test");
        assert_eq!(
            slugify("Plano de Teste — Migração InfluxDB → TDengine"),
            "plano-de-teste-migracao-influx-db-t-dengine"
        );
        assert_eq!(slugify("myURLstring"), "my-ur-lstring");
        assert_eq!(slugify("ADR 0001"), "adr-0001");
        assert_eq!(slugify("  Hello,   World!  "), "hello-world");
        assert_eq!(slugify("♥"), "");
    }

    #[test]
    fn builds_page_urls_with_the_slug_id_last() {
        assert_eq!(
            page_url(
                "https://wiki.example.com",
                "general",
                "sZ6xIJ2hMh",
                Some("docmost-cli smoke test")
            ),
            "https://wiki.example.com/s/general/p/docmost-cli-smoke-test-sZ6xIJ2hMh"
        );
        assert_eq!(
            page_url("https://wiki.example.com/", "general", "abc", None),
            "https://wiki.example.com/s/general/p/untitled-abc"
        );
        let long = "x".repeat(100);
        let url = page_url("http://h", "s", "id", Some(&long));
        assert_eq!(url, format!("http://h/s/s/p/{}-id", "x".repeat(70)));
    }
}
