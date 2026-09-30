//! Shared XML reader: no DTD, no custom entities, bounded depth and nodes.

use crate::{Cancel, Limits, Refusal};

/// A flattened XML event. Element names are local names (prefix removed).
/// Attribute keys keep their prefix exactly as written (`r:id`, `w:val`,
/// `ContentType`), so an attribute in a foreign namespace can never stand in
/// for an unprefixed one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Open {
        name: String,
        attributes: Vec<(String, String)>,
    },
    Close,
    Text(String),
}

/// Parses UTF-8 XML into flat events. Refuses any DTD, every entity except
/// the five predefined ones and character references, characters and names
/// outside XML 1.0, `--` in comments, `]]>` in text, `<` in attribute
/// values, more than one root, and open elements at end of input. Depth and
/// node counts are BRAN's own, so nesting never recurses.
pub fn parse(bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Vec<Event>, Refusal> {
    use quick_xml::events::Event as Q;
    cancel.check()?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| Refusal::MalformedXml)?;
    if !text.chars().all(is_xml_char) {
        return Err(Refusal::MalformedXml);
    }
    let mut reader = quick_xml::reader::NsReader::from_str(text);
    reader.config_mut().check_comments = true;
    let mut events = Vec::new();
    let (mut depth, mut nodes, mut roots) = (0usize, 0usize, 0usize);
    loop {
        let event = reader.read_event().map_err(|_| Refusal::MalformedXml)?;
        let empty = matches!(event, Q::Empty(_));
        match event {
            Q::Start(element) | Q::Empty(element) => {
                if matches!(
                    reader.resolver().resolve_element(element.name()).0,
                    quick_xml::name::ResolveResult::Unknown(_)
                ) {
                    return Err(Refusal::MalformedXml);
                }
                if depth == 0 {
                    roots += 1;
                }
                if roots > 1 {
                    return Err(Refusal::MalformedXml);
                }
                if depth + 1 > limits.max_xml_depth {
                    return Err(Refusal::XmlDepthLimit);
                }
                count(&mut nodes, limits)?;
                if !is_qualified_name(element.name().as_ref()) {
                    return Err(Refusal::MalformedXml);
                }
                let mut attributes = Vec::new();
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|_| Refusal::MalformedXml)?;
                    if matches!(
                        reader.resolver().resolve_attribute(attribute.key).0,
                        quick_xml::name::ResolveResult::Unknown(_)
                    ) {
                        return Err(Refusal::MalformedXml);
                    }
                    let key: &str = attribute.key.as_ref();
                    if !is_qualified_name(key) || attribute.value.contains('<') {
                        return Err(Refusal::MalformedXml);
                    }
                    let value = attribute
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| Refusal::MalformedXml)?;
                    if !value.chars().all(is_xml_char) {
                        return Err(Refusal::MalformedXml);
                    }
                    attributes.push((key.to_owned(), value.into_owned()));
                }
                let name = element.local_name();
                events.push(Event::Open {
                    name: AsRef::<str>::as_ref(&name).to_owned(),
                    attributes,
                });
                if empty {
                    events.push(Event::Close);
                } else {
                    depth += 1;
                }
            }
            Q::End(_) => {
                depth = depth.checked_sub(1).ok_or(Refusal::MalformedXml)?;
                events.push(Event::Close);
            }
            Q::Text(content) if content.contains("]]>") => return Err(Refusal::MalformedXml),
            Q::Text(content) => text_event(
                &mut events,
                &content.xml10_content(),
                depth,
                &mut nodes,
                limits,
            )?,
            Q::CData(content) => text_event(&mut events, &content, depth, &mut nodes, limits)?,
            Q::GeneralRef(reference) => {
                let resolved = if reference.is_char_ref() {
                    reference
                        .resolve_char_ref()
                        .ok()
                        .flatten()
                        .map(String::from)
                } else {
                    quick_xml::escape::resolve_predefined_entity(&reference).map(str::to_owned)
                };
                let resolved = resolved
                    .filter(|text| text.chars().all(is_xml_char))
                    .ok_or(Refusal::MalformedXml)?;
                text_event(&mut events, &resolved, depth, &mut nodes, limits)?;
            }
            Q::DocType(_) => return Err(Refusal::XmlDtdRefused),
            Q::Decl(_) | Q::PI(_) | Q::Comment(_) => {}
            Q::Eof => {
                if depth != 0 || roots != 1 {
                    return Err(Refusal::MalformedXml);
                }
                return Ok(events);
            }
        }
    }
}

fn count(nodes: &mut usize, limits: &Limits) -> Result<(), Refusal> {
    *nodes += 1;
    if *nodes > limits.max_xml_nodes {
        return Err(Refusal::XmlNodeLimit);
    }
    Ok(())
}

/// Appends text, merging adjacent pieces. Only whitespace may sit outside the
/// root element.
fn text_event(
    events: &mut Vec<Event>,
    text: &str,
    depth: usize,
    nodes: &mut usize,
    limits: &Limits,
) -> Result<(), Refusal> {
    if depth == 0 {
        return if text.trim().is_empty() {
            Ok(())
        } else {
            Err(Refusal::MalformedXml)
        };
    }
    if let Some(Event::Text(previous)) = events.last_mut() {
        previous.push_str(text);
        return Ok(());
    }
    count(nodes, limits)?;
    events.push(Event::Text(text.to_owned()));
    Ok(())
}

/// XML 1.0 `Char`. Rust `char` already excludes surrogates.
fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// XML 1.0 `Name` restricted by Namespaces in XML: at most one colon, and
/// neither the prefix nor the local part may be empty.
fn is_qualified_name(name: &str) -> bool {
    let mut parts = name.split(':');
    let valid = |part: &str| {
        let mut chars = part.chars();
        chars.next().is_some_and(is_name_start) && chars.all(is_name_char)
    };
    match (parts.next(), parts.next(), parts.next()) {
        (Some(local), None, None) => valid(local),
        (Some(prefix), Some(local), None) => valid(prefix) && valid(local),
        _ => false,
    }
}

fn is_name_start(c: char) -> bool {
    matches!(c,
        'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}'
        | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}'
        | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}
