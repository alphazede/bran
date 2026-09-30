//! Shared XML reader: no DTD, no custom entities, bounded depth and nodes.

use crate::{Cancel, Limits, Refusal};

/// A flattened XML event. Names are local names (prefix removed).
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
/// the five predefined ones and character references, more than one root,
/// and open elements at end of input. Depth and node counts are BRAN's own,
/// so nesting never recurses.
pub fn parse(bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Vec<Event>, Refusal> {
    use quick_xml::events::Event as Q;
    cancel.check()?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| Refusal::MalformedXml)?;
    let mut reader = quick_xml::Reader::from_str(text);
    let mut events = Vec::new();
    let (mut depth, mut nodes, mut roots) = (0usize, 0usize, 0usize);
    loop {
        let event = reader.read_event().map_err(|_| Refusal::MalformedXml)?;
        let empty = matches!(event, Q::Empty(_));
        match event {
            Q::Start(element) | Q::Empty(element) => {
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
                let mut attributes = Vec::new();
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|_| Refusal::MalformedXml)?;
                    let value = attribute
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| Refusal::MalformedXml)?;
                    let key = attribute.key.local_name();
                    attributes.push((AsRef::<str>::as_ref(&key).to_owned(), value.into_owned()));
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
                let resolved = resolved.ok_or(Refusal::MalformedXml)?;
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
