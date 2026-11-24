//! `presentation/protobuf_writer.py`: building ProPresenter 19 presentations from a slide plan.
//!
//! Everything here works on bytes in and bytes out, so it is exactly testable; [`crate::files`] adds the folder work (backup, lock, atomic
//! replace). Random ids come from a [`UuidSource`] and the clock is a parameter, so a run can be reproduced byte for byte against Python.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use versevision_core::config::PresentationTemplateConfig;
use versevision_core::plan::{SlideKind, SlidePlan};

use crate::compat::CompatibilityPolicy;
use crate::error::{PpError, Result};
use crate::model::{
    self, base_slide_mut, casefold, element_name, first_chars, read_presentation,
    slide_action_indices, uuid_of, NIL_UUID,
};
use crate::pb::{Message, Value};
use crate::rtf::{replace_rtf_text, visible_length};
use crate::themes::{self, LoadedLooks, EXTRA, MARKER};

/// Where new ids come from: each call is one `uuid.uuid4()` (lowercase, hyphenated).
pub trait UuidSource {
    fn next_uuid(&mut self) -> String;
}

impl<F: FnMut() -> String> UuidSource for F {
    fn next_uuid(&mut self) -> String {
        self()
    }
}

/// Random version-4 ids.
#[derive(Debug, Default, Clone, Copy)]
pub struct RandomUuids;

impl UuidSource for RandomUuids {
    fn next_uuid(&mut self) -> String {
        uuid::Uuid::new_v4().hyphenated().to_string()
    }
}

/// The ids the oracle uses: the n-th id of a run is the version-4 uuid made from `sha256("<seed>:<n>")` (n counts from 0).
/// `python/scripts/oracle_pro.py` makes the same ids on the Python side.
#[derive(Debug, Clone)]
pub struct SeededUuids {
    seed: String,
    counter: u64,
}

impl SeededUuids {
    pub fn new(seed: &str) -> Self {
        SeededUuids {
            seed: seed.to_string(),
            counter: 0,
        }
    }
}

impl UuidSource for SeededUuids {
    fn next_uuid(&mut self) -> String {
        let digest = Sha256::digest(format!("{}:{}", self.seed, self.counter).as_bytes());
        self.counter += 1;
        let mut bytes: [u8; 16] = digest[..16].try_into().expect("16 bytes");
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        uuid::Uuid::from_bytes(bytes).hyphenated().to_string()
    }
}

/// What a write did (`PlanWriteResult`, without the paths).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanWriteResult {
    pub cue_uuids: Vec<String>,
    pub cue_count_before: usize,
    pub cue_count_after: usize,
    pub group_count_before: usize,
    pub group_count_after: usize,
    pub output_sha256: String,
    pub original_sha256: String,
    pub removed_cue_count: usize,
}

/// A presentation file's bytes with what was done to make it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub bytes: Vec<u8>,
    pub result: PlanWriteResult,
}

/// `WriteResult` of the feasibility slide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntheticResult {
    pub cue_uuid: String,
    pub cue_count_before: usize,
    pub cue_count_after: usize,
    pub output_sha256: String,
}

/// `PresentationInfo` (`inspect`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationInfo {
    pub name: String,
    pub uuid: String,
    pub cue_count: usize,
    pub group_count: usize,
    pub application_version: String,
    pub document_version: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ------------------------------------------------------------------------------------------------------------------ text

/// `_set_text`: put new text into a text object and keep its character-range formatting true to the new text.
///
/// A ProPresenter text object can carry formatting for a RANGE of characters (all caps for characters 0-180, a font scale, ...). The range is
/// as long as the template's sample text, so text longer than the sample ran past it and the last words came out in lowercase. Any range that
/// reached the end of the old text now reaches the end of the new text; a range that stopped earlier is only clipped to the new length.
pub fn set_text(text: &mut Message, new_text: &str) -> Result<()> {
    let old_length = visible_length(text.bytes("rtf_data"))? as i64;
    let rtf = replace_rtf_text(text.bytes("rtf_data"), new_text)?;
    text.set_bytes("rtf_data", rtf);
    let new_length = new_text.encode_utf16().count() as i64;
    if text.msg("attributes").is_some() {
        for custom in text.msg_mut("attributes").msgs_mut("custom_attributes") {
            let (start, end) = custom
                .msg("range")
                .map(|r| (i64::from(r.i32("start")), i64::from(r.i32("end"))))
                .unwrap_or((0, 0));
            if end >= old_length - 1 {
                custom.msg_mut("range").set_int("end", new_length);
            } else if end > new_length {
                custom
                    .msg_mut("range")
                    .set_int("end", start.max(new_length));
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------------------------ uuids

fn is_uuid_field(field: &prost_reflect::FieldDescriptor) -> Option<bool> {
    Message::field_message_type(field).map(|m| m.full_name() == "rv.data.UUID")
}

fn messages_mut(value: &mut Value) -> Vec<&mut Message> {
    match value {
        Value::Msg(m) => vec![m],
        Value::List(items) => items.iter_mut().filter_map(Value::as_msg_mut).collect(),
        _ => Vec::new(),
    }
}

fn messages(value: &Value) -> Vec<&Message> {
    match value {
        Value::Msg(m) => vec![m],
        Value::List(items) => items.iter().filter_map(Value::as_msg).collect(),
        _ => Vec::new(),
    }
}

/// `_refresh_uuids`: every id in the message gets a new random one (the nil id stays). Returns the new ids.
pub fn refresh_uuids(message: &mut Message, uuids: &mut dyn UuidSource) -> BTreeSet<String> {
    let mut generated = BTreeSet::new();
    refresh_into(message, uuids, &mut generated);
    generated
}

fn refresh_into(
    message: &mut Message,
    uuids: &mut dyn UuidSource,
    generated: &mut BTreeSet<String>,
) {
    for (field, value) in message.list_fields_mut() {
        let Some(is_uuid) = is_uuid_field(&field) else {
            continue;
        };
        for item in messages_mut(value) {
            if is_uuid {
                let current = item.str("string");
                if !current.is_empty() && current != NIL_UUID {
                    let fresh = uuids.next_uuid();
                    item.set_str("string", &fresh);
                    generated.insert(fresh);
                }
            } else {
                refresh_into(item, uuids, generated);
            }
        }
    }
}

/// `_all_uuids`: every id in the message, in order.
pub fn all_uuids(message: &Message) -> Vec<String> {
    let mut found = Vec::new();
    collect_uuids(message, &mut found);
    found
}

fn collect_uuids(message: &Message, found: &mut Vec<String>) {
    for (field, value) in message.list_fields() {
        let Some(is_uuid) = is_uuid_field(&field) else {
            continue;
        };
        for item in messages(value) {
            if is_uuid {
                let s = item.str("string");
                if !s.is_empty() && s != NIL_UUID {
                    found.push(s.to_string());
                }
            } else {
                collect_uuids(item, found);
            }
        }
    }
}

/// `_remap_owned_ids`: give every id inside a copied presentation a new (upper-case) value, keeping references between them consistent.
/// Python draws a new id for EVERY id it meets, also for one it has already mapped (the draw is the argument of `setdefault`); so does this,
/// otherwise a seeded run would drift.
pub fn remap_owned_ids(
    message: &mut Message,
    uuids: &mut dyn UuidSource,
) -> BTreeMap<String, String> {
    let mut mapping = BTreeMap::new();
    remap_into(message, uuids, &mut mapping);
    mapping
}

fn remap_into(
    message: &mut Message,
    uuids: &mut dyn UuidSource,
    mapping: &mut BTreeMap<String, String>,
) {
    for (field, value) in message.list_fields_mut() {
        let Some(is_uuid) = is_uuid_field(&field) else {
            continue;
        };
        for item in messages_mut(value) {
            if is_uuid {
                let current = item.str("string").to_string();
                if !current.is_empty() && current != NIL_UUID {
                    let drawn = uuids.next_uuid().to_uppercase();
                    let mapped = mapping.entry(casefold(&current)).or_insert(drawn).clone();
                    item.set_str("string", &mapped);
                }
            } else {
                remap_into(item, uuids, mapping);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ cues and groups

/// `_cue_and_group`: the template cue (a copy), its cue group (a copy) and the index of its slide action.
fn cue_and_group(message: &Message, cue_uuid: &str) -> Result<(Message, Message, usize)> {
    let wanted = casefold(cue_uuid);
    let cue = message
        .msgs("cues")
        .into_iter()
        .find(|c| casefold(uuid_of(c)) == wanted)
        .ok_or_else(|| PpError::value("configured template cue no longer exists"))?;
    let slides = slide_action_indices(cue);
    if slides.len() != 1 {
        return Err(PpError::value(
            "template cue must contain exactly one slide action",
        ));
    }
    let own = uuid_of(cue);
    let group = message
        .msgs("cue_groups")
        .into_iter()
        .find(|g| {
            g.msgs("cue_identifiers")
                .iter()
                .any(|i| i.str("string") == own)
        })
        .ok_or_else(|| PpError::value("template cue is not assigned to a cue group"))?;
    Ok((cue.clone(), group.clone(), slides[0]))
}

fn text_elements_named(action: &Message, name: &str) -> usize {
    model::elements_of_action(action)
        .iter()
        .filter(|e| element_name(e) == name && model::has_text(e))
        .count()
}

/// `_template_parts`
fn template_parts(
    message: &Message,
    definition: &PresentationTemplateConfig,
) -> Result<(Message, Message)> {
    let (cue, group, index) = cue_and_group(message, &definition.cue_uuid)?;
    let action = cue.msgs("actions")[index];
    if text_elements_named(action, &definition.reference_object) != 1
        || text_elements_named(action, &definition.body_object) != 1
    {
        return Err(PpError::value(
            "template must contain exactly one configured reference and verse object",
        ));
    }
    Ok((cue, group))
}

fn first_slide_action_mut(cue: &mut Message) -> Result<&mut Message> {
    let index = slide_action_indices(cue)
        .first()
        .copied()
        .ok_or_else(|| PpError::value("cue has no slide action"))?;
    Ok(cue.msgs_mut("actions").swap_remove(index))
}

/// The wrapper (`Slide.Element`) of the first element with this name (Python's `next(item for item in elements if item.element.name == name)`).
fn named_element_mut<'a>(elements: &'a mut [&mut Message], name: &str) -> Result<&'a mut Message> {
    elements
        .iter_mut()
        .find(|e| element_name(e) == name)
        .map(|e| &mut **e)
        .ok_or_else(|| PpError::value(format!("template has no object named {name}")))
}

/// `_render_plan`: append a cue for each planned slide (and a group for each passage) to `original`, cloning the templates in `source`.
/// Returns the new cue ids in order.
pub fn render_plan(
    original: &mut Message,
    plan: &SlidePlan,
    definition: &PresentationTemplateConfig,
    source: &Message,
    looks: Option<&LoadedLooks>,
    uuids: &mut dyn UuidSource,
) -> Result<Vec<String>> {
    let mut verse_parts: Option<(Message, Message)> = None;
    let mut point_parts: Option<(Message, Message)> = None;
    if plan.slides.iter().any(|s| s.kind == SlideKind::Verse) {
        verse_parts = Some(template_parts(source, definition)?);
    }
    if plan.slides.iter().any(|s| s.kind == SlideKind::Point) {
        let point_uuid = definition
            .point_cue_uuid
            .as_deref()
            .filter(|u| !u.is_empty())
            .ok_or_else(|| {
                PpError::value(
                    "no sermon-point template is configured; run versevision template select-point",
                )
            })?;
        let (cue, group, index) = cue_and_group(source, point_uuid)?;
        if text_elements_named(cue.msgs("actions")[index], &definition.point_object) != 1 {
            return Err(PpError::value(
                "sermon-point template must contain exactly one configured point text object",
            ));
        }
        point_parts = Some((cue, group));
    }
    let mut existing: BTreeSet<String> = all_uuids(original).into_iter().collect();
    let mut created: Vec<String> = Vec::new();
    let mut grouped: Vec<(usize, Vec<String>)> = Vec::new();
    let mut group_labels: BTreeMap<usize, String> = BTreeMap::new();
    let mut group_templates: BTreeMap<usize, Message> = BTreeMap::new();
    for planned in &plan.slides {
        let is_point = planned.kind == SlideKind::Point;
        let (template_cue, template_group) = if is_point {
            point_parts.as_ref()
        } else {
            verse_parts.as_ref()
        }
        .expect("template parts were checked");
        let mut cue = template_cue.clone();
        let look = looks.and_then(|l| {
            if is_point {
                l.point.as_ref()
            } else {
                l.verse.as_ref()
            }
        });
        let mut has_reference_box = true;
        if let Some(look) = look {
            let action = first_slide_action_mut(&mut cue)?;
            has_reference_box = themes::apply_look(
                action,
                look,
                is_point,
                &definition.reference_object,
                &definition.body_object,
                &definition.point_object,
            )?;
            cue.set_str("name", MARKER);
        }
        let new_uuids = refresh_uuids(&mut cue, uuids);
        if existing.intersection(&new_uuids).next().is_some() {
            return Err(PpError::value(
                "new cue UUID collides with the presentation",
            ));
        }
        existing.extend(new_uuids);
        let cue_uuid = uuid_of(&cue).to_string();
        let action = first_slide_action_mut(&mut cue)?;
        let label;
        if is_point {
            label = first_chars(&planned.body_text.replace('\n', " "), 60);
            action.msg_mut("label").set_str("text", &label);
            let mut elements = base_slide_mut(action).msgs_mut("elements");
            let body = named_element_mut(&mut elements, &definition.point_object)?;
            set_text(body.msg_mut("element").msg_mut("text"), &planned.body_text)?;
        } else {
            label = if planned.version_text.is_empty() {
                planned.reference_text.clone()
            } else {
                format!("{} ({})", planned.reference_text, planned.version_text)
            };
            action.msg_mut("label").set_str("text", &label);
            let mut elements = base_slide_mut(action).msgs_mut("elements");
            if has_reference_box {
                let reference = named_element_mut(&mut elements, &definition.reference_object)?;
                set_text(reference.msg_mut("element").msg_mut("text"), &label)?;
                let body = named_element_mut(&mut elements, &definition.body_object)?;
                set_text(body.msg_mut("element").msg_mut("text"), &planned.body_text)?;
            } else {
                // a one-box theme: the reference is the last line of the same box
                let body = named_element_mut(&mut elements, &definition.body_object)?;
                set_text(
                    body.msg_mut("element").msg_mut("text"),
                    &format!("{}\n{label}", planned.body_text),
                )?;
            }
        }
        if look.is_some() {
            let mut elements =
                base_slide_mut(first_slide_action_mut(&mut cue)?).msgs_mut("elements");
            for item in elements.iter_mut() {
                if element_name(item) == EXTRA {
                    set_text(item.msg_mut("element").msg_mut("text"), "")?;
                }
            }
        }
        original.push("cues", cue);
        created.push(cue_uuid.clone());
        match grouped
            .iter_mut()
            .find(|(i, _)| *i == planned.source_passage_index)
        {
            Some((_, list)) => list.push(cue_uuid),
            None => grouped.push((planned.source_passage_index, vec![cue_uuid])),
        }
        group_labels
            .entry(planned.source_passage_index)
            .or_insert(label);
        group_templates
            .entry(planned.source_passage_index)
            .or_insert_with(|| template_group.clone());
    }
    for (passage_index, cue_uuids) in grouped {
        let mut group = group_templates[&passage_index].clone();
        refresh_uuids(group.msg_mut("group"), uuids);
        let label = &group_labels[&passage_index];
        group.msg_mut("group").set_str("name", label);
        group
            .msg_mut("group")
            .set_str("application_group_name", label);
        group.clear_list("cue_identifiers");
        for cue_uuid in cue_uuids {
            group.add("cue_identifiers").set_str("string", &cue_uuid);
        }
        original.push("cue_groups", group);
    }
    Ok(created)
}

/// `_remove_cues`: remove cues and drop them from groups; groups left empty are removed along with their arrangement references.
/// Returns (cues removed, groups removed).
pub fn remove_cues(message: &mut Message, cue_uuids: &BTreeSet<String>) -> (usize, usize) {
    let wanted: BTreeSet<String> = cue_uuids.iter().map(|u| casefold(u)).collect();
    let before = message.len("cues");
    let kept: Vec<Message> = message
        .msgs("cues")
        .into_iter()
        .filter(|c| !wanted.contains(&casefold(uuid_of(c))))
        .cloned()
        .collect();
    let removed = before - kept.len();
    if removed > 0 {
        message.clear_list("cues");
        for cue in kept {
            message.push("cues", cue);
        }
    }
    let mut removed_groups: BTreeSet<String> = BTreeSet::new();
    let mut surviving: Vec<Message> = Vec::new();
    for group in message.msgs("cue_groups") {
        let mut group = group.clone();
        let identifiers: Vec<String> = group
            .msgs("cue_identifiers")
            .iter()
            .map(|i| i.str("string").to_string())
            .filter(|s| !wanted.contains(&casefold(s)))
            .collect();
        if identifiers.len() != group.len("cue_identifiers") {
            group.clear_list("cue_identifiers");
            for identifier in &identifiers {
                group.add("cue_identifiers").set_str("string", identifier);
            }
            if identifiers.is_empty() {
                removed_groups.insert(
                    group
                        .msg("group")
                        .map(|g| uuid_of(g).to_string())
                        .unwrap_or_default(),
                );
                continue;
            }
        }
        surviving.push(group);
    }
    // Python edits the surviving groups in place whether or not a group disappeared, and swaps the list only when one did; either way
    // the groups that stay are the edited ones.
    message.clear_list("cue_groups");
    for group in surviving {
        message.push("cue_groups", group);
    }
    if !removed_groups.is_empty() {
        for arrangement in message.msgs_mut("arrangements") {
            let keep: Vec<String> = arrangement
                .msgs("group_identifiers")
                .iter()
                .map(|i| i.str("string").to_string())
                .filter(|s| !removed_groups.contains(s))
                .collect();
            arrangement.clear_list("group_identifiers");
            for identifier in keep {
                arrangement
                    .add("group_identifiers")
                    .set_str("string", &identifier);
            }
        }
    }
    (removed, removed_groups.len())
}

/// `generated_cue_uuids`: cues that hold only a verse (Reference + Verse objects) or a sermon point, never blanks or media.
pub fn generated_cue_uuids(
    message: &Message,
    definition: &PresentationTemplateConfig,
) -> Vec<String> {
    let mut wanted: Vec<Vec<String>> = Vec::new();
    for names in [
        vec![
            definition.reference_object.clone(),
            definition.body_object.clone(),
        ],
        vec![definition.point_object.clone()],
    ] {
        let mut names = names;
        names.sort();
        wanted.push(names);
    }
    let body_only = vec![definition.body_object.clone()];
    let protected: BTreeSet<String> = [
        Some(definition.cue_uuid.as_str()),
        definition.point_cue_uuid.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|u| !u.is_empty())
    .map(casefold)
    .collect();
    let mut found = Vec::new();
    for cue in message.msgs("cues") {
        if protected.contains(&casefold(uuid_of(cue))) {
            continue; // the template slides themselves are never "generated" slides
        }
        let actions = cue.msgs("actions");
        let slides: Vec<&&Message> = actions.iter().filter(|a| a.has("slide")).collect();
        if slides.len() != 1 || actions.len() != 1 {
            continue;
        }
        let elements = model::elements_of_action(slides[0]);
        let mut texts: Vec<String> = elements
            .iter()
            .filter(|e| model::has_text(e))
            .map(|e| element_name(e).to_string())
            .collect();
        texts.sort();
        if model::cue_name(cue) == MARKER {
            // built from a ProPresenter theme: may also carry the theme's shapes or background, and a one-box theme has no Reference object
            let names: Vec<String> = texts.iter().filter(|n| *n != EXTRA).cloned().collect();
            if wanted.contains(&names) || names == body_only {
                found.push(uuid_of(cue).to_string());
            }
            continue;
        }
        if texts.len() != elements.len() {
            continue; // has a shape/image/other object: not a plain generated slide
        }
        if wanted.contains(&texts) {
            found.push(uuid_of(cue).to_string());
        }
    }
    found
}

fn stamp_now(presentation: &mut Message, now: i64) {
    for field in ["last_date_used", "last_modified_date"] {
        let stamp = presentation.msg_mut(field);
        stamp.set_int("seconds", now);
        stamp.set_int("nanos", 0);
    }
}

fn passage_count(plan: &SlidePlan) -> usize {
    plan.slides
        .iter()
        .map(|s| s.source_passage_index)
        .collect::<BTreeSet<_>>()
        .len()
}

/// `inspect`.
pub fn inspect(bytes: &[u8]) -> Result<PresentationInfo> {
    let message = read_presentation(bytes)?;
    let info = message.msg("application_info");
    let version = |name: &str, field: &str| {
        info.and_then(|i| i.msg(name))
            .map(|v| v.u32(field))
            .unwrap_or(0)
    };
    let build = |name: &str| {
        info.and_then(|i| i.msg(name))
            .map(|v| v.str("build").to_string())
            .unwrap_or_default()
    };
    Ok(PresentationInfo {
        name: message.str("name").to_string(),
        uuid: uuid_of(&message).to_string(),
        cue_count: message.len("cues"),
        group_count: message.len("cue_groups"),
        application_version: format!(
            "{}.{}.{} ({})",
            version("application_version", "major_version"),
            version("application_version", "minor_version"),
            version("application_version", "patch_version"),
            build("application_version")
        ),
        document_version: format!(
            "{}/{}",
            version("platform_version", "major_version"),
            build("platform_version")
        ),
    })
}

/// Builds presentations. `uuids` is where new ids come from.
pub struct PresentationWriter<U: UuidSource = RandomUuids> {
    pub policy: CompatibilityPolicy,
    pub uuids: U,
}

impl Default for PresentationWriter<RandomUuids> {
    fn default() -> Self {
        PresentationWriter {
            policy: CompatibilityPolicy::default(),
            uuids: RandomUuids,
        }
    }
}

impl<U: UuidSource> PresentationWriter<U> {
    pub fn new(uuids: U) -> Self {
        PresentationWriter {
            policy: CompatibilityPolicy::default(),
            uuids,
        }
    }

    /// `append_plan`: append a plan to a presentation, optionally removing an earlier import's cues in the same write.
    /// `template` is the presentation holding the template slides; `None` when they are in `original` itself (they are read BEFORE any slide is
    /// removed, otherwise clearing the old verse/point slides would delete the template being cloned).
    pub fn append_plan(
        &mut self,
        original_bytes: &[u8],
        plan: &SlidePlan,
        definition: &PresentationTemplateConfig,
        looks: Option<&LoadedLooks>,
        template_bytes: Option<&[u8]>,
        remove_cue_uuids: Option<&[String]>,
    ) -> Result<Written> {
        let remove = remove_cue_uuids.filter(|r| !r.is_empty());
        if plan.slides.is_empty() && remove.is_none() {
            return Err(PpError::value("slide plan is empty"));
        }
        let original_hash = sha256_hex(original_bytes);
        let mut original = read_presentation(original_bytes)?;
        self.policy.validate(&original)?;
        let template = match template_bytes {
            Some(bytes) => {
                let template = read_presentation(bytes)?;
                self.policy.validate(&template)?;
                template
            }
            None => original.clone(),
        };
        let (before_cues, before_groups) = (original.len("cues"), original.len("cue_groups"));
        let (mut removed_cues, mut removed_groups) = (0, 0);
        if let Some(remove) = remove {
            (removed_cues, removed_groups) =
                remove_cues(&mut original, &remove.iter().cloned().collect());
        }
        let cue_uuids = if plan.slides.is_empty() {
            Vec::new()
        } else {
            render_plan(
                &mut original,
                plan,
                definition,
                &template,
                looks,
                &mut self.uuids,
            )?
        };
        let added_groups = passage_count(plan);
        let expected_cues = before_cues - removed_cues + cue_uuids.len();
        let expected_groups = before_groups - removed_groups + added_groups;
        let bytes = original.encode();
        let reparsed = read_presentation(&bytes)?;
        if reparsed.len("cues") != expected_cues {
            return Err(PpError::value("temporary presentation cue count differs"));
        }
        if reparsed.len("cue_groups") != expected_groups {
            return Err(PpError::value("temporary presentation group count differs"));
        }
        if !cue_uuids.is_empty() {
            let cues = reparsed.msgs("cues");
            let appended: BTreeSet<&str> = cues[cues.len() - cue_uuids.len()..]
                .iter()
                .map(|c| uuid_of(c))
                .collect();
            let wanted: BTreeSet<&str> = cue_uuids.iter().map(String::as_str).collect();
            if appended != wanted {
                return Err(PpError::value(
                    "temporary presentation is missing generated cue UUIDs",
                ));
            }
        }
        let result = PlanWriteResult {
            cue_uuids,
            cue_count_before: before_cues,
            cue_count_after: expected_cues,
            group_count_before: before_groups,
            group_count_after: expected_groups,
            output_sha256: sha256_hex(&bytes),
            original_sha256: original_hash,
            removed_cue_count: removed_cues,
        };
        Ok(Written { bytes, result })
    }

    /// `replace_generated`: remove VerseVision-style verse and sermon-point cues (keeping every other slide, such as blanks, backgrounds, titles
    /// and media) and append the planned slides.
    pub fn replace_generated(
        &mut self,
        original_bytes: &[u8],
        plan: &SlidePlan,
        definition: &PresentationTemplateConfig,
        looks: Option<&LoadedLooks>,
        template_bytes: Option<&[u8]>,
    ) -> Result<Written> {
        if plan.slides.is_empty() {
            return Err(PpError::value("slide plan is empty"));
        }
        let original = read_presentation(original_bytes)?;
        let victims = generated_cue_uuids(&original, definition);
        self.append_plan(
            original_bytes,
            plan,
            definition,
            looks,
            template_bytes,
            Some(&victims),
        )
    }

    /// `create_presentation`: a brand-new presentation named `name` from the template presentation: the template's application/document headers,
    /// background and timeline, no cues of its own, every planned slide.
    pub fn create_presentation(
        &mut self,
        template_bytes: &[u8],
        plan: &SlidePlan,
        definition: &PresentationTemplateConfig,
        looks: Option<&LoadedLooks>,
        name: &str,
        now: i64,
    ) -> Result<Written> {
        if plan.slides.is_empty() {
            return Err(PpError::value("slide plan is empty"));
        }
        let template = read_presentation(template_bytes)?;
        self.policy.validate(&template)?;
        let mut shell = template.clone();
        refresh_uuids(&mut shell, &mut self.uuids); // new presentation id (and no id shared with the template)
        shell.set_str("name", name);
        shell.clear_list("cues");
        shell.clear_list("cue_groups");
        shell.clear_list("arrangements");
        shell.clear("selected_arrangement");
        stamp_now(&mut shell, now);
        let created = render_plan(
            &mut shell,
            plan,
            definition,
            &template,
            looks,
            &mut self.uuids,
        )?;
        let bytes = shell.encode();
        let reparsed = read_presentation(&bytes)?;
        if reparsed.len("cues") != created.len() || reparsed.str("name") != name {
            return Err(PpError::value("new presentation did not reread correctly"));
        }
        let result = PlanWriteResult {
            cue_count_before: 0,
            cue_count_after: created.len(),
            group_count_before: 0,
            group_count_after: passage_count(plan),
            output_sha256: sha256_hex(&bytes),
            original_sha256: String::new(),
            removed_cue_count: 0,
            cue_uuids: created,
        };
        Ok(Written { bytes, result })
    }

    /// `create_template_file`: a small presentation holding ONLY the verse and sermon-point template slides of the source. The template slides
    /// keep their ids (so the saved configuration stays valid); the file itself is a new presentation.
    pub fn create_template_file(
        &mut self,
        source_bytes: &[u8],
        definition: &PresentationTemplateConfig,
        name: &str,
        now: i64,
    ) -> Result<Written> {
        let source = read_presentation(source_bytes)?;
        self.policy.validate(&source)?;
        let wanted: Vec<String> = [
            Some(definition.cue_uuid.as_str()),
            definition.point_cue_uuid.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .collect();
        let present: BTreeSet<String> = source
            .msgs("cues")
            .iter()
            .map(|c| casefold(uuid_of(c)))
            .collect();
        let missing: Vec<&String> = wanted
            .iter()
            .filter(|u| !present.contains(&casefold(u)))
            .collect();
        if !missing.is_empty() {
            return Err(PpError::value(format!(
                "the source presentation no longer holds the template slide(s): {}",
                missing
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let mut copy = source.clone();
        let wanted_folded: BTreeSet<String> = wanted.iter().map(|u| casefold(u)).collect();
        let others: BTreeSet<String> = copy
            .msgs("cues")
            .iter()
            .map(|c| uuid_of(c).to_string())
            .filter(|u| !wanted_folded.contains(&casefold(u)))
            .collect();
        remove_cues(&mut copy, &others);
        copy.clear_list("arrangements");
        copy.clear("selected_arrangement");
        let fresh = self.uuids.next_uuid().to_uppercase();
        model::set_uuid(&mut copy, &fresh);
        copy.set_str("name", name);
        stamp_now(&mut copy, now);
        let bytes = copy.encode();
        let reparsed = read_presentation(&bytes)?;
        let reread: BTreeSet<String> = reparsed
            .msgs("cues")
            .iter()
            .map(|c| casefold(uuid_of(c)))
            .collect();
        if reread != wanted_folded {
            return Err(PpError::value("the template file did not reread correctly"));
        }
        let result = PlanWriteResult {
            cue_uuids: wanted.clone(),
            cue_count_before: source.len("cues"),
            cue_count_after: wanted.len(),
            group_count_before: source.len("cue_groups"),
            group_count_after: copy.len("cue_groups"),
            output_sha256: sha256_hex(&bytes),
            original_sha256: sha256_hex(source_bytes),
            removed_cue_count: 0,
        };
        Ok(Written { bytes, result })
    }

    /// `derive_presentation`: the bytes of a COPY of the source with the planned slides added (a new file, so ProPresenter lists it at once).
    /// `clear_generated` first drops the copy's verse and sermon-point slides (blank, background, title and media slides stay). Every id inside
    /// the copy is new, so it never collides with the source; `keep_uuid` keeps the presentation's own id (a swap: playlists point at it).
    #[allow(clippy::too_many_arguments)]
    pub fn derive_presentation(
        &mut self,
        source_bytes: &[u8],
        template_bytes: Option<&[u8]>,
        plan: &SlidePlan,
        definition: &PresentationTemplateConfig,
        looks: Option<&LoadedLooks>,
        name: &str,
        clear_generated: bool,
        keep_uuid: bool,
        now: i64,
    ) -> Result<Written> {
        if plan.slides.is_empty() {
            return Err(PpError::value("slide plan is empty"));
        }
        let source = read_presentation(source_bytes)?;
        self.policy.validate(&source)?;
        let template = match template_bytes {
            Some(bytes) => {
                let template = read_presentation(bytes)?;
                self.policy.validate(&template)?;
                template
            }
            None => source.clone(), // snapshot BEFORE anything is cleared: the template may live in this file
        };
        let mut copy = source.clone();
        let mut removed_cues = 0;
        if clear_generated {
            let victims = generated_cue_uuids(&copy, definition);
            if !victims.is_empty() {
                removed_cues = remove_cues(&mut copy, &victims.into_iter().collect()).0;
            }
        }
        let (before_cues, before_groups) = (copy.len("cues"), copy.len("cue_groups"));
        let original_uuid = uuid_of(&copy).to_string();
        remap_owned_ids(&mut copy, &mut self.uuids);
        if keep_uuid && !original_uuid.is_empty() {
            model::set_uuid(&mut copy, &original_uuid); // a swap: playlists point at this id, so the new version keeps it
        }
        copy.set_str("name", name);
        stamp_now(&mut copy, now);
        let created = render_plan(
            &mut copy,
            plan,
            definition,
            &template,
            looks,
            &mut self.uuids,
        )?;
        let expected = before_cues + created.len();
        let bytes = copy.encode();
        let reparsed = read_presentation(&bytes)?;
        if reparsed.len("cues") != expected || reparsed.str("name") != name {
            return Err(PpError::value("new presentation did not reread correctly"));
        }
        let cues = reparsed.msgs("cues");
        let appended: BTreeSet<&str> = cues[cues.len() - created.len()..]
            .iter()
            .map(|c| uuid_of(c))
            .collect();
        if appended != created.iter().map(String::as_str).collect::<BTreeSet<_>>() {
            return Err(PpError::value(
                "new presentation is missing the generated slides",
            ));
        }
        let result = PlanWriteResult {
            cue_count_before: source.len("cues"),
            cue_count_after: expected,
            group_count_before: source.len("cue_groups"),
            group_count_after: before_groups + passage_count(plan),
            output_sha256: sha256_hex(&bytes),
            original_sha256: sha256_hex(source_bytes),
            removed_cue_count: removed_cues,
            cue_uuids: created,
        };
        Ok(Written { bytes, result })
    }

    /// `append_synthetic_slide`: the feasibility slide: a copy of the first cue and group with one text.
    pub fn append_synthetic_slide(
        &mut self,
        original_bytes: &[u8],
        body_text: &str,
        label: &str,
    ) -> Result<(Vec<u8>, SyntheticResult)> {
        let mut original = read_presentation(original_bytes)?;
        self.policy.validate(&original)?;
        if original.len("cues") == 0 || original.len("cue_groups") == 0 {
            return Err(PpError::value(
                "presentation has no cue and group to use as a template",
            ));
        }
        let before = original.len("cues");
        let existing: BTreeSet<String> = all_uuids(&original).into_iter().collect();
        let mut cue = original.msgs("cues")[0].clone();
        let new_uuids = refresh_uuids(&mut cue, &mut self.uuids);
        if existing.intersection(&new_uuids).next().is_some() {
            return Err(PpError::value(
                "new cue UUID collides with the presentation",
            ));
        }
        let cue_uuid = uuid_of(&cue).to_string();
        cue.set_str("name", label);
        let slides = slide_action_indices(&cue);
        if slides.len() != 1 {
            return Err(PpError::value(
                "template cue must contain exactly one slide action",
            ));
        }
        let action = cue.msgs_mut("actions").swap_remove(slides[0]);
        let mut elements = base_slide_mut(action).msgs_mut("elements");
        let mut text_indexes: Vec<usize> = (0..elements.len())
            .filter(|i| model::has_text(elements[*i]))
            .collect();
        if text_indexes.len() != 1 {
            return Err(PpError::value(
                "feasibility template must contain exactly one text element",
            ));
        }
        let index = text_indexes.remove(0);
        set_text(
            elements[index].msg_mut("element").msg_mut("text"),
            body_text,
        )?;
        let mut group = original.msgs("cue_groups")[0].clone();
        let group_uuid = self.uuids.next_uuid();
        model::set_uuid(group.msg_mut("group"), &group_uuid);
        let identifier_is_nil = group
            .msg("group")
            .and_then(|g| g.msg("application_group_identifier"))
            .map(|i| i.str("string") == NIL_UUID)
            .unwrap_or(false);
        if !identifier_is_nil {
            let fresh = self.uuids.next_uuid();
            group
                .msg_mut("group")
                .msg_mut("application_group_identifier")
                .set_str("string", &fresh);
        }
        group.msg_mut("group").set_str("name", label);
        group
            .msg_mut("group")
            .set_str("application_group_name", label);
        group.clear_list("cue_identifiers");
        group.add("cue_identifiers").set_str("string", &cue_uuid);
        original.push("cue_groups", group);
        original.push("cues", cue);
        let bytes = original.encode();
        let reparsed = read_presentation(&bytes)?;
        if reparsed.len("cues") != before + 1
            || reparsed.msgs("cues").last().map(|c| uuid_of(c)) != Some(cue_uuid.as_str())
        {
            return Err(PpError::value(
                "temporary presentation failed structural verification",
            ));
        }
        let result = SyntheticResult {
            cue_uuid,
            cue_count_before: before,
            cue_count_after: before + 1,
            output_sha256: sha256_hex(&bytes),
        };
        self.verify_synthetic(&bytes, &result)?;
        Ok((bytes, result))
    }

    /// `verify`: the written file reparses, still passes the compatibility check, has the expected cue count and last cue, and no id is repeated
    /// other than a cue id listed in a group.
    pub fn verify_synthetic(&self, bytes: &[u8], expected: &SyntheticResult) -> Result<()> {
        let fail = |m: &str| Err(PpError::value(m));
        let message = read_presentation(bytes)?;
        self.policy.validate(&message)?;
        if message.len("cues") != expected.cue_count_after {
            return fail("cue count differs");
        }
        if message.msgs("cues").last().map(|c| uuid_of(c)) != Some(expected.cue_uuid.as_str()) {
            return fail("appended cue UUID differs");
        }
        let uuids = all_uuids(&message);
        let permitted: BTreeSet<&str> = message
            .msgs("cue_groups")
            .iter()
            .flat_map(|g| g.msgs("cue_identifiers"))
            .map(|i| i.str("string"))
            .collect();
        let duplicated: BTreeSet<&str> = uuids
            .iter()
            .map(String::as_str)
            .filter(|u| uuids.iter().filter(|x| x.as_str() == *u).count() > 1)
            .collect();
        if duplicated.difference(&permitted).next().is_some() {
            return fail("unexpected duplicate object UUID");
        }
        if sha256_hex(bytes) != expected.output_sha256 {
            return fail("output hash changed after write");
        }
        Ok(())
    }
}
