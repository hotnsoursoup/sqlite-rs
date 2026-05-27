pub(crate) fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

pub(crate) fn quote_qualified_identifier(identifier: &str) -> Option<String> {
    let parts: Vec<_> = identifier.split('.').map(str::trim).collect();

    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        return None;
    }

    Some(
        parts
            .into_iter()
            .map(quote_identifier)
            .collect::<Vec<_>>()
            .join("."),
    )
}

pub(crate) fn quote_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_identifier_escapes_embedded_quotes() {
        assert_eq!(quote_identifier("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn quote_qualified_identifier_quotes_each_segment() {
        assert_eq!(
            quote_qualified_identifier("main.users").unwrap(),
            "\"main\".\"users\""
        );
    }

    #[test]
    fn quote_qualified_identifier_rejects_empty_segments() {
        assert!(quote_qualified_identifier("").is_none());
        assert!(quote_qualified_identifier("main.").is_none());
        assert!(quote_qualified_identifier("main..users").is_none());
    }
}
