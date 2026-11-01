//! Multimodal image serialization shared by the provider adapters (design §7.2).
//!
//! `ModelImage` carries MIME type plus raw base64 so each family can render its
//! own wire shape; the only shared wire concept is the openai-compatible data
//! URL, which lives here so no adapter reimplements the prefix rule.

use ark_ports::ModelImage;

/// Encode one image as an openai-compatible `data:` URL.
pub fn data_url(image: &ModelImage) -> String {
    format!("data:{};base64,{}", image.mime_type, image.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_url_carries_mime_and_base64_payload() {
        let image = ModelImage {
            mime_type: "image/png".into(),
            data: "aGVsbG8=".into(),
        };
        assert_eq!(data_url(&image), "data:image/png;base64,aGVsbG8=");
    }
}
