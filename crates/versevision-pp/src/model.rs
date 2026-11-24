//! Reading the parts of a presentation the writer works with: cues, their slide action, the elements of the slide.

use crate::error::{PpError, Result};
use crate::pb::Message;

pub const PRESENTATION: &str = "rv.data.Presentation";
pub const NIL_UUID: &str = "00000000-0000-0000-0000-000000000000";

pub fn read_presentation(bytes: &[u8]) -> Result<Message> {
    Ok(Message::decode_named(PRESENTATION, bytes)?)
}

/// Indexes of the actions of a cue that are slide actions (`WhichOneof("ActionTypeData") == "slide"`).
pub fn slide_action_indices(cue: &Message) -> Vec<usize> {
    cue.msgs("actions")
        .iter()
        .enumerate()
        .filter(|(_, a)| a.has("slide"))
        .map(|(i, _)| i)
        .collect()
}

/// The slide of an action: `action.slide.presentation.base_slide`.
pub fn base_slide(action: &Message) -> Option<&Message> {
    action.msg("slide")?.msg("presentation")?.msg("base_slide")
}

pub fn base_slide_mut(action: &mut Message) -> &mut Message {
    action
        .msg_mut("slide")
        .msg_mut("presentation")
        .msg_mut("base_slide")
}

/// The wrappers (`Slide.Element`) of a slide action.
pub fn elements_of_action(action: &Message) -> Vec<&Message> {
    base_slide(action)
        .map(|b| b.msgs("elements"))
        .unwrap_or_default()
}

/// `_slide_elements(cue)`: the elements of the cue's only slide action; none when it does not have exactly one.
pub fn slide_elements(cue: &Message) -> Vec<&Message> {
    let actions = cue.msgs("actions");
    let slides: Vec<_> = actions.iter().filter(|a| a.has("slide")).collect();
    if slides.len() != 1 {
        return Vec::new();
    }
    elements_of_action(slides[0])
}

pub fn element(wrapper: &Message) -> Option<&Message> {
    wrapper.msg("element")
}

pub fn element_name(wrapper: &Message) -> &str {
    element(wrapper).map(|e| e.str("name")).unwrap_or("")
}

pub fn has_text(wrapper: &Message) -> bool {
    element(wrapper).is_some_and(|e| e.has("text"))
}

/// A cue's own uuid string.
pub fn uuid_of(message: &Message) -> &str {
    message.msg("uuid").map(|u| u.str("string")).unwrap_or("")
}

pub fn set_uuid(message: &mut Message, value: &str) {
    message.msg_mut("uuid").set_str("string", value);
}

pub fn cue_name(cue: &Message) -> &str {
    cue.str("name")
}

/// `(x, y, width, height)` of an element's bounds.
pub fn rect(wrapper: &Message) -> (f64, f64, f64, f64) {
    let bounds = element(wrapper).and_then(|e| e.msg("bounds"));
    let origin = bounds.and_then(|b| b.msg("origin"));
    let size = bounds.and_then(|b| b.msg("size"));
    let get = |m: Option<&Message>, name: &str| m.map(|m| m.float(name)).unwrap_or(0.0);
    (
        get(origin, "x"),
        get(origin, "y"),
        get(size, "width"),
        get(size, "height"),
    )
}

/// Find a cue by uuid (any case) in a presentation.
pub fn find_cue<'a>(presentation: &'a Message, cue_uuid: &str) -> Option<&'a Message> {
    let wanted = casefold(cue_uuid);
    presentation
        .msgs("cues")
        .into_iter()
        .find(|c| casefold(uuid_of(c)) == wanted)
}

pub fn casefold(text: &str) -> String {
    versevision_core::references::casefold(text)
}

/// `str(value)[:limit]`-style slicing by code points.
pub fn first_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// A presentation the writer was given that cannot be a template or a target.
pub fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(PpError::value(message))
    }
}
