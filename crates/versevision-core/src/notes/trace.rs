//! Where the classifier writes its trace records (`runlog.trace(stage, **fields)` in Python).

use serde_json::{Map, Value};

/// Python's `_clean(value, limit)` for the trace: strings longer than the limit are cut and say how much was removed, lists keep their
/// first 300 items, and a key that looks like a secret has the value `"[REDACTED]"`.
pub fn clean_value(value: Value, limit: usize) -> Value {
    const SECRET_WORDS: [&str; 7] = [
        "api_key",
        "apikey",
        "authorization",
        "password",
        "secret",
        "token",
        "x-api-key",
    ];
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let folded = crate::references::pytext::casefold(&k);
                    let secret = SECRET_WORDS.iter().any(|w| folded.contains(w));
                    let v = if secret {
                        Value::String("[REDACTED]".to_string())
                    } else {
                        clean_value(v, limit)
                    };
                    (k, v)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .take(300)
                .map(|v| clean_value(v, limit))
                .collect(),
        ),
        Value::String(text) => {
            let count = text.chars().count();
            if count > limit {
                let head: String = text.chars().take(limit).collect();
                Value::String(format!("{head}... [{} more characters]", count - limit))
            } else {
                Value::String(text)
            }
        }
        other => other,
    }
}

/// Receives the decision records in the order they are written. The planner (S3) numbers them (`seq`) and writes them to `trace.jsonl`.
pub trait TraceSink {
    /// One record: the stage name and its fields (already cleaned the way the Python run log does).
    fn trace(&mut self, stage: &str, fields: Map<String, Value>);
}

/// Throw the records away (a run that does not keep a trace).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoTrace;

impl TraceSink for NoTrace {
    fn trace(&mut self, _stage: &str, _fields: Map<String, Value>) {}
}

/// Keep the records.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TraceLog {
    pub records: Vec<(String, Map<String, Value>)>,
}

impl TraceSink for TraceLog {
    fn trace(&mut self, stage: &str, fields: Map<String, Value>) {
        self.records.push((stage.to_string(), fields));
    }
}

impl TraceLog {
    /// The records as the lines of `trace.jsonl` would hold them, numbered from `first_seq`.
    pub fn to_json(&self, first_seq: u64) -> Vec<Value> {
        self.records
            .iter()
            .enumerate()
            .map(|(i, (stage, fields))| {
                let mut record = Map::new();
                record.insert("seq".to_string(), Value::from(first_seq + i as u64));
                record.insert("stage".to_string(), Value::String(stage.clone()));
                for (k, v) in fields {
                    record.insert(k.clone(), v.clone());
                }
                Value::Object(record)
            })
            .collect()
    }
}
