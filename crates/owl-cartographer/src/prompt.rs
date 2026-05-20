//! LLM prompt templates for entity / relation extraction.

/// System prompt instructing the model to return strict JSON.
pub const EXTRACTION_SYSTEM: &str = "\
You are an information-extraction engine. Read the user's text and return a strict JSON \
object with two keys: `entities` and `relations`. Do not include any other prose.

Schema:
{
  \"entities\": [
    { \"id\": \"<lowercase_canonical_name>\",
      \"name\": \"<surface form>\",
      \"kind\": \"person|org|place|concept|event|product|other\",
      \"description\": \"<one short sentence>\" }
  ],
  \"relations\": [
    { \"source\": \"<entity id>\",
      \"target\": \"<entity id>\",
      \"label\": \"<verb-like phrase>\",
      \"weight\": 0.0 to 1.0 }
  ]
}

Be conservative: only emit relations for facts explicitly stated in the text.\
";

/// Build the user-side extraction prompt for a chunk of text.
pub fn extraction_user(chunk: &str) -> String {
    format!("Extract entities and relations from the following text:\n\n---\n{chunk}\n---")
}

// ─── Dual-level keyword extraction (LightRAG-style) ───────────────────────────

/// System prompt instructing the model to split a query into low- and high-level
/// keyword sets — the seed lists for hybrid (graph + vector) retrieval. Loaded
/// from `prompts/keyword_extraction.md`.
pub const KEYWORD_EXTRACTION_SYSTEM: &str = include_str!("../prompts/keyword_extraction.md");

/// Build the user-side keyword-extraction prompt for a query string.
pub fn keyword_user(query: &str) -> String {
    format!("Question:\n{query}\n\nReturn the JSON object now.")
}
