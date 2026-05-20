//! LLM-driven entity + relation extraction.

use rig::agent::AgentBuilder;
use rig::completion::Prompt;

use owl_protocol::graph::ExtractionResult;

use crate::prompt::{extraction_user, EXTRACTION_SYSTEM};
use crate::CartographerError;

/// Run extraction over a single chunk of text using the provided rig model.
pub async fn extract<M>(model: M, chunk: &str) -> Result<ExtractionResult, CartographerError>
where
    M: rig::completion::CompletionModel + Clone + 'static,
{
    let agent = AgentBuilder::new(model).preamble(EXTRACTION_SYSTEM).build();
    let raw = agent
        .prompt(extraction_user(chunk).as_str())
        .await
        .map_err(|e| CartographerError::Extraction(e.to_string()))?;

    parse_extraction(&raw)
}

/// Parse the LLM's response, tolerating leading/trailing prose by extracting the first
/// balanced `{...}` block.
fn parse_extraction(raw: &str) -> Result<ExtractionResult, CartographerError> {
    let json = first_json_object(raw)
        .ok_or_else(|| CartographerError::InvalidResponse("no JSON object found".into()))?;
    serde_json::from_str::<ExtractionResult>(json)
        .map_err(|e| CartographerError::InvalidResponse(e.to_string()))
}

/// Return the substring of the first balanced `{...}` block, or None.
fn first_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth = 0i32;
    for (i, ch) in s[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::first_json_object;

    #[test]
    fn extracts_first_balanced_object() {
        let s = "noise {\"a\": {\"b\": 1}} trailing";
        assert_eq!(first_json_object(s), Some("{\"a\": {\"b\": 1}}"));
    }
}
