//! Wire JSON — the small shapes every adapter spells the same way.

/// `data:image/png;base64,…` — how the OpenAI schemas carry image bytes inside a JSON string.
pub(crate) fn data_url(mime: &str, data: &str) -> String {
    format!("data:{mime};base64,{data}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_url_names_the_mime_and_the_encoding() {
        assert_eq!(data_url("image/png", "QUJD"), "data:image/png;base64,QUJD");
    }
}
