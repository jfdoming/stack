pub fn url_encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push_str(&format!("{:02X}", b));
        }
    }
    out
}

pub fn github_owner_from_web_url(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    let (_, rest) = trimmed.split_once("://")?;
    let mut parts = rest.split('/');
    let _host = parts.next()?;
    let owner = parts.next()?;
    if owner.is_empty() {
        return None;
    }
    Some(owner.to_string())
}

pub fn github_repo_slug_from_web_url(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    let (_, rest) = trimmed.split_once("://")?;
    let mut parts = rest.split('/');
    let _host = parts.next()?;
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_repo_slug_from_web_url_parses_owner_and_repo() {
        let slug = github_repo_slug_from_web_url("https://github.com/acme/repo")
            .expect("repo slug should parse");
        assert_eq!(slug, "acme/repo");
    }
}
