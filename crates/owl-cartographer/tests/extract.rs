//! Integration tests for `owl-cartographer` extraction pipeline.
//!
//! Run with: `cargo test -p owl-cartographer`

use owl_cartographer::extract;
use owl_harness::mock_engine::MockEngine;

// ── parse_extraction (unit-level, exposed via extract module) ─────────────────

#[tokio::test]
async fn mock_extraction_roundtrip() {
    // The mock engine returns a well-formed JSON response as the LLM would.
    let response_json = r#"{"entities":[{"id":"alice","name":"Alice","kind":"person","description":"A developer"}],"relations":[]}"#;
    let model = MockEngine::new().on("any", response_json);

    let result = extract::extract(model, "Alice is a developer.").await.unwrap();
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].name, "Alice");
    assert_eq!(result.entities[0].kind, "person");
    assert!(result.relations.is_empty());
}

#[tokio::test]
async fn extraction_tolerates_leading_prose() {
    // LLMs sometimes wrap JSON in prose — the extractor should still parse it.
    let response = r#"Sure, here is the result: {"entities":[{"id":"x","name":"X","kind":"concept","description":"X thing"}],"relations":[]}"#;
    let model = MockEngine::new().on("text", response);

    let result = extract::extract(model, "X is a concept.").await.unwrap();
    assert_eq!(result.entities[0].id, "x");
}

#[tokio::test]
async fn extraction_fails_gracefully_on_invalid_json() {
    let model = MockEngine::new().on("bad", "sorry I don't know");
    let result = extract::extract(model, "garbage input").await;
    assert!(result.is_err(), "should return an error when JSON is missing");
}

#[tokio::test]
async fn extraction_with_relations() {
    let json = r#"{
        "entities": [
            {"id":"alice","name":"Alice","kind":"person","description":"dev"},
            {"id":"repo","name":"Knight-Owl","kind":"project","description":"agent"}
        ],
        "relations": [
            {"source":"alice","target":"repo","label":"maintains","weight":0.9}
        ]
    }"#;
    let model = MockEngine::new().on("text", json);
    let result = extract::extract(model, "Alice maintains Knight-Owl.").await.unwrap();
    assert_eq!(result.entities.len(), 2);
    assert_eq!(result.relations.len(), 1);
    assert_eq!(result.relations[0].label, "maintains");
    assert!((result.relations[0].weight - 0.9).abs() < 1e-4);
}
