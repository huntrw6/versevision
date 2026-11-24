//! `rapidfuzz.fuzz.WRatio` and `process.extract` (the near-miss hints of "Presentation target was not found") against `oracle/pipeline/selection.jsonl`.

use std::path::Path;

use versevision_pp::selection::{extract, wratio};

#[test]
fn wratio_and_extract_are_what_rapidfuzz_answers() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle/pipeline/selection.jsonl");
    let text = std::fs::read_to_string(path).unwrap();
    let (mut scores, mut tops) = (0usize, 0usize);
    for line in text.lines() {
        let case: serde_json::Value = serde_json::from_str(line).unwrap();
        let query = case["query"].as_str().unwrap();
        let choices: Vec<String> = case["choices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        for (choice, expected) in case["scores"].as_object().unwrap() {
            let got = wratio(query, choice);
            assert!(
                (got - expected.as_f64().unwrap()).abs() < 1e-6,
                "WRatio({query:?}, {choice:?}): expected {expected}, got {got}"
            );
            scores += 1;
        }
        let top = case["top"].as_array().unwrap();
        if !top.is_empty() {
            let got = extract(query, &choices, 3);
            assert_eq!(got.len(), top.len(), "extract({query:?}) length");
            for (g, e) in got.iter().zip(top) {
                assert_eq!(g.0, e[0].as_str().unwrap(), "extract({query:?}) order");
                assert!(
                    (g.1 - e[1].as_f64().unwrap()).abs() < 1e-6,
                    "extract({query:?}) score of {}",
                    g.0
                );
                assert_eq!(g.2, e[2].as_u64().unwrap() as usize);
            }
            tops += 1;
        }
    }
    println!("selection oracle: {scores} scores and {tops} top-3 lists match rapidfuzz");
    assert!(scores > 400 && tops >= 20);
}
