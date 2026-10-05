//! Path normalization for route matching (RFC 3986 §6.2.2).
//!
//! Matching uses the normalized form so equivalent spellings select the same
//! route; the request is forwarded with its original target.

use std::borrow::Cow;

/// Decodes percent-encoded unreserved characters, uppercases the remaining
/// percent-encodings and removes dot segments. `None` for non-absolute paths.
pub fn normalize(path: &str) -> Option<Cow<'_, str>> {
    if !path.starts_with('/') {
        return None;
    }
    if !path.contains('%') && !has_dot_segment(path) {
        return Some(Cow::Borrowed(path));
    }
    let decoded = decode_unreserved(path);
    Some(Cow::Owned(remove_dot_segments(&decoded)))
}

fn has_dot_segment(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "." || segment == "..")
}

fn decode_unreserved(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut output = String::with_capacity(path.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                let byte = high << 4 | low;
                if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                    output.push(char::from(byte));
                } else {
                    output.push('%');
                    output.push(char::from(bytes[index + 1].to_ascii_uppercase()));
                    output.push(char::from(bytes[index + 2].to_ascii_uppercase()));
                }
                index += 3;
                continue;
            }
        }
        let character = path[index..]
            .chars()
            .next()
            .expect("index is a char boundary");
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// RFC 3986 §5.2.4 for an absolute path.
fn remove_dot_segments(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    let mut trailing_slash = false;
    for segment in path.split('/').skip(1) {
        trailing_slash = false;
        match segment {
            "." => trailing_slash = true,
            ".." => {
                segments.pop();
                trailing_slash = true;
            }
            segment => segments.push(segment),
        }
    }
    let mut output = String::with_capacity(path.len());
    for segment in &segments {
        output.push('/');
        output.push_str(segment);
    }
    if trailing_slash || output.is_empty() {
        output.push('/');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn plain_paths_are_borrowed() {
        assert!(matches!(
            normalize("/api/users").unwrap(),
            std::borrow::Cow::Borrowed("/api/users")
        ));
        assert!(normalize("relative").is_none());
        assert!(normalize("").is_none());
    }

    #[test]
    fn unreserved_percent_encodings_are_decoded() {
        assert_eq!(normalize("/%61pi/%7Euser").unwrap(), "/api/~user");
        assert_eq!(normalize("/a%2fb").unwrap(), "/a%2Fb");
        assert_eq!(normalize("/space%20x").unwrap(), "/space%20x");
        assert_eq!(normalize("/bad%zz").unwrap(), "/bad%zz");
        assert_eq!(normalize("/end%4").unwrap(), "/end%4");
        assert_eq!(normalize("/ü%41").unwrap(), "/üA");
    }

    #[test]
    fn dot_segments_follow_rfc_3986() {
        assert_eq!(normalize("/a/b/c/./../../g").unwrap(), "/a/g");
        assert_eq!(normalize("/../../etc/passwd").unwrap(), "/etc/passwd");
        assert_eq!(normalize("/a/b/..").unwrap(), "/a/");
        assert_eq!(normalize("/a/.").unwrap(), "/a/");
        assert_eq!(normalize("/%2e%2E/admin").unwrap(), "/admin");
        assert_eq!(normalize("/..").unwrap(), "/");
        assert_eq!(normalize("/a//b").unwrap(), "/a//b");
    }
}
