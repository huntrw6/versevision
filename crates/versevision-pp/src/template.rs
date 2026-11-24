//! `presentation/template.py`: finding the template slides in a presentation, the fingerprint of their style, and checking a saved template
//! definition against the file.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use versevision_core::config::PresentationTemplateConfig;

use crate::error::{PpError, Result};
use crate::model::{casefold, element, element_name, has_text, slide_elements, uuid_of};
use crate::pb::{Message, Value};
use crate::rtf::plain_rtf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateCandidate {
    pub slide_number: usize,
    pub cue_uuid: String,
    pub reference_object_uuid: String,
    pub body_object_uuid: String,
    pub reference_preview: String,
    pub body_preview: String,
    pub style_fingerprint: String,
}

/// `_unsign_zeros`: -0.0 and 0.0 are the same number (different machines write the same style with either sign).
fn unsign_zeros(message: &mut Message) {
    let mut clear = Vec::new();
    for (field, value) in message.list_fields_mut() {
        match value {
            Value::Msg(inner) => unsign_zeros(inner),
            Value::List(items) => {
                for item in items {
                    match item {
                        Value::Msg(inner) => unsign_zeros(inner),
                        Value::F32(v) if *v == 0.0 => *v = 0.0,
                        Value::F64(v) if *v == 0.0 => *v = 0.0,
                        _ => {}
                    }
                }
            }
            Value::F32(v) if *v == 0.0 => {
                if field.supports_presence() {
                    *v = 0.0;
                } else {
                    clear.push(field.number());
                }
            }
            Value::F64(v) if *v == 0.0 => {
                if field.supports_presence() {
                    *v = 0.0;
                } else {
                    clear.push(field.number());
                }
            }
            _ => {}
        }
    }
    for number in clear {
        message.remove_number(number);
    }
}

/// `_fingerprint_bytes(wrapper)`: the element with its text emptied and its zeros unsigned, serialised.
pub fn fingerprint_bytes(wrapper: &Message) -> Vec<u8> {
    let mut clone = wrapper.clone();
    clone
        .msg_mut("element")
        .msg_mut("text")
        .set_bytes("rtf_data", Vec::new());
    unsign_zeros(&mut clone);
    clone.encode()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `_style_fingerprint(reference, body)`.
pub fn style_fingerprint(reference: &Message, body: &Message) -> String {
    let mut digest = Sha256::new();
    digest.update(fingerprint_bytes(reference));
    digest.update(fingerprint_bytes(body));
    hex(&digest.finalize())
}

fn named_texts<'a>(elements: &[&'a Message], name: &str) -> Vec<&'a Message> {
    elements
        .iter()
        .copied()
        .filter(|e| element_name(e) == name && has_text(e))
        .collect()
}

fn preview(wrapper: &Message, limit: usize) -> String {
    let rtf = element(wrapper)
        .and_then(|e| e.msg("text"))
        .map(|t| t.bytes("rtf_data"))
        .unwrap_or(&[]);
    plain_rtf(rtf).chars().take(limit).collect()
}

/// `inspect_template_candidates`: every cue with exactly one reference text object and one body text object.
pub fn inspect_template_candidates(
    presentation: &Message,
    reference_object: &str,
    body_object: &str,
) -> Vec<TemplateCandidate> {
    let mut found = Vec::new();
    for (index, cue) in presentation.msgs("cues").into_iter().enumerate() {
        let elements = slide_elements(cue);
        let references = named_texts(&elements, reference_object);
        let bodies = named_texts(&elements, body_object);
        if references.len() != 1 || bodies.len() != 1 {
            continue;
        }
        let (reference, body) = (references[0], bodies[0]);
        found.push(TemplateCandidate {
            slide_number: index + 1,
            cue_uuid: uuid_of(cue).to_string(),
            reference_object_uuid: element(reference).map(uuid_of).unwrap_or("").to_string(),
            body_object_uuid: element(body).map(uuid_of).unwrap_or("").to_string(),
            reference_preview: preview(reference, 100),
            body_preview: preview(body, 160),
            style_fingerprint: style_fingerprint(reference, body),
        });
    }
    found
}

/// `inspect_point_candidates`: cues with exactly one text element, named `point_object`: (slide number, cue uuid, fingerprint).
pub fn inspect_point_candidates(
    presentation: &Message,
    point_object: &str,
) -> Vec<(usize, String, String)> {
    let mut found = Vec::new();
    for (index, cue) in presentation.msgs("cues").into_iter().enumerate() {
        let elements = slide_elements(cue);
        let points = named_texts(&elements, point_object);
        if points.len() != 1
            || elements
                .iter()
                .any(|e| has_text(e) && !std::ptr::eq(*e, points[0]))
        {
            continue;
        }
        found.push((
            index + 1,
            uuid_of(cue).to_string(),
            hex(&Sha256::digest(fingerprint_bytes(points[0]))),
        ));
    }
    found
}

/// `create_template_definition`: the config entry for the chosen cue.
pub fn create_template_definition(
    presentation: &Message,
    presentation_uuid: &str,
    presentation_name: &str,
    library_name: Option<&str>,
    cue_uuid: &str,
    reference_object: &str,
    body_object: &str,
) -> Result<PresentationTemplateConfig> {
    let candidate = inspect_template_candidates(presentation, reference_object, body_object)
        .into_iter()
        .find(|c| casefold(&c.cue_uuid) == casefold(cue_uuid))
        .ok_or_else(|| {
            PpError::configuration(format!(
                "Cue {cue_uuid} is not a valid {reference_object}/{body_object} template"
            ))
        })?;
    Ok(PresentationTemplateConfig {
        presentation_uuid: presentation_uuid.to_string(),
        presentation_name: presentation_name.to_string(),
        library_name: library_name.map(str::to_string),
        cue_uuid: candidate.cue_uuid,
        reference_object: reference_object.to_string(),
        body_object: body_object.to_string(),
        style_fingerprint: candidate.style_fingerprint,
        point_cue_uuid: None,
        point_object: "Sermon Point".to_string(),
        point_style_fingerprint: None,
    })
}

/// `validate_template`: the saved definition still describes the file.
pub fn validate_template(
    presentation: &Message,
    definition: &PresentationTemplateConfig,
) -> Result<TemplateCandidate> {
    let candidate = inspect_template_candidates(
        presentation,
        &definition.reference_object,
        &definition.body_object,
    )
    .into_iter()
    .find(|c| casefold(&c.cue_uuid) == casefold(&definition.cue_uuid))
    .ok_or_else(|| {
        PpError::configuration("Configured template cue or named text objects no longer exist")
    })?;
    if candidate.style_fingerprint != definition.style_fingerprint {
        return Err(PpError::configuration(
            "Configured template styling or geometry changed; select the template again",
        ));
    }
    if let Some(point_uuid) = definition
        .point_cue_uuid
        .as_deref()
        .filter(|u| !u.is_empty())
    {
        let point = inspect_point_candidates(presentation, &definition.point_object)
            .into_iter()
            .find(|p| casefold(&p.1) == casefold(point_uuid))
            .ok_or_else(|| PpError::configuration("Configured sermon-point template cue no longer exists; run template select-point"))?;
        if let Some(saved) = definition
            .point_style_fingerprint
            .as_deref()
            .filter(|f| !f.is_empty())
        {
            if point.2 != saved {
                return Err(PpError::configuration(
                    "Sermon-point template styling changed; run template select-point again",
                ));
            }
        }
    }
    Ok(candidate)
}
