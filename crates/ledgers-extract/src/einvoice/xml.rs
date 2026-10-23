use super::EInvoiceError;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;
pub struct Node {
    pub name: String,
    pub ns: String,
    pub path: String,
    attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<usize>,
}
impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct Doc {
    nodes: Vec<Node>,
}
impl Doc {
    pub fn parse(bytes: &[u8]) -> Result<Self, EInvoiceError> {
        if bytes.len() > super::MAX_XML_BYTES {
            return Err(EInvoiceError::TooLarge);
        }
        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().trim_text(true);
        let mut nodes: Vec<Node> = Vec::new();
        let mut open: Vec<(usize, HashMap<String, usize>)> = Vec::new();
        let mut buf = Vec::new();
        let mut root_seen = false;
        loop {
            match reader.read_event_into(&mut buf) {
                Err(_) => return Err(EInvoiceError::Malformed("XML read error".into())),
                Ok(Event::Start(e)) => {
                    if root_seen && open.is_empty() {
                        return Err(EInvoiceError::Malformed("multiple root elements".into()));
                    }
                    if open.len() >= super::MAX_DEPTH {
                        return Err(EInvoiceError::TooDeep);
                    }
                    if nodes.len() >= super::MAX_NODES {
                        return Err(EInvoiceError::TooManyNodes);
                    }
                    root_seen = true;
                    add_node(&mut nodes, &mut open, &e, root_seen)?;
                    open.push((nodes.len() - 1, HashMap::new()));
                }
                Ok(Event::Empty(e)) => {
                    if root_seen && open.is_empty() {
                        return Err(EInvoiceError::Malformed("multiple root elements".into()));
                    }
                    if open.len() >= super::MAX_DEPTH {
                        return Err(EInvoiceError::TooDeep);
                    }
                    if nodes.len() >= super::MAX_NODES {
                        return Err(EInvoiceError::TooManyNodes);
                    }
                    root_seen = true;
                    add_node(&mut nodes, &mut open, &e, root_seen)?;
                }
                Ok(Event::End(_)) => {
                    open.pop();
                }
                Ok(Event::Text(t)) => {
                    if let Ok(text) = t.decode() {
                        append(&mut nodes, &open, &text)?;
                    }
                }
                Ok(Event::CData(c)) => {
                    if let Ok(text) = c.decode() {
                        append(&mut nodes, &open, &text)?;
                    }
                }
                Ok(Event::GeneralRef(r)) => {
                    if let Ok(name) = r.decode() {
                        if let Some(s) = quick_xml::escape::resolve_predefined_entity(name.as_ref())
                        {
                            append(&mut nodes, &open, s)?;
                        } else {
                            return Err(EInvoiceError::UnknownEntity);
                        }
                    } else {
                        return Err(EInvoiceError::UnknownEntity);
                    }
                }
                Ok(Event::DocType(_)) => return Err(EInvoiceError::Doctype),
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        if !open.is_empty() || nodes.is_empty() {
            return Err(EInvoiceError::Malformed(
                "unclosed or empty document".into(),
            ));
        }
        Ok(Self { nodes })
    }
    pub fn root(&self) -> &Node {
        &self.nodes[0]
    }
    pub fn child(&self, node: &Node, name: &str) -> Option<&Node> {
        self.children(node, name).next()
    }
    pub fn children<'a>(&'a self, node: &Node, name: &str) -> impl Iterator<Item = &'a Node> {
        let nodes = &self.nodes;
        node.children
            .iter()
            .map(move |&i| &nodes[i])
            .filter(move |c| c.name == name)
    }
    pub fn path<'a>(&'a self, node: &'a Node, names: &[&str]) -> Option<&'a Node> {
        names.iter().try_fold(node, |n, name| self.child(n, name))
    }
}
fn add_node(
    nodes: &mut Vec<Node>,
    open: &mut [(usize, HashMap<String, usize>)],
    e: &quick_xml::events::BytesStart,
    _root: bool,
) -> Result<(), EInvoiceError> {
    let local = std::str::from_utf8(e.local_name().as_ref())
        .map_err(|_| EInvoiceError::Malformed("bad name".into()))?
        .to_owned();
    let prefix = std::str::from_utf8(e.name().as_ref())
        .ok()
        .and_then(|s| s.split(':').next())
        .map(String::from)
        .unwrap_or_default();
    let mut ns = String::new();
    let mut attrs = Vec::new();
    for attr_res in e.attributes() {
        let attr = attr_res.map_err(|_| EInvoiceError::Malformed("bad attr".into()))?;
        let key = std::str::from_utf8(attr.key.as_ref())
            .unwrap_or("")
            .to_owned();
        let raw = std::str::from_utf8(&attr.value).map_err(|_| EInvoiceError::UnknownEntity)?;
        let val = quick_xml::escape::unescape(raw)
            .map_err(|_| EInvoiceError::UnknownEntity)?
            .to_string();
        if key == "xmlns" || (!prefix.is_empty() && key == format!("xmlns:{prefix}")) {
            ns.clone_from(&val);
        }
        attrs.push((key, val));
    }
    let path = if let Some((pi, cs)) = open.last_mut() {
        let c = cs.entry(local.clone()).or_insert(0);
        *c += 1;
        format!("{}/{local}[{}]", nodes[*pi].path, c)
    } else {
        format!("/{local}")
    };
    let id = nodes.len();
    nodes.push(Node {
        name: local,
        ns,
        path,
        attrs,
        text: String::new(),
        children: Vec::new(),
    });
    if let Some((pi, _)) = open.last() {
        nodes[*pi].children.push(id);
    }
    Ok(())
}
fn append(
    nodes: &mut [Node],
    open: &[(usize, HashMap<String, usize>)],
    text: &str,
) -> Result<(), EInvoiceError> {
    if text.trim().is_empty() {
        return if open.is_empty() {
            Err(EInvoiceError::Malformed("text outside root".into()))
        } else {
            Ok(())
        };
    }
    if let Some((node_idx, _)) = open.last() {
        nodes[*node_idx].text.push_str(text);
        Ok(())
    } else {
        Err(EInvoiceError::Malformed("text outside root".into()))
    }
}
pub fn decimal(text: &str, exponent: u32) -> Option<i64> {
    let t = text.trim();
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (int, frac) = t.split_once('.').unwrap_or((t, ""));
    let digits = usize::try_from(exponent).ok()?;
    if (int.is_empty() && frac.is_empty())
        || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let (kept, extra) = frac.split_at(frac.len().min(digits));
    if extra.bytes().any(|b| b != b'0') {
        return None;
    }
    let value = int
        .bytes()
        .chain(kept.bytes())
        .chain(std::iter::repeat_n(b'0', digits - kept.len()))
        .try_fold(0i64, |v, b| {
            v.checked_mul(10)?.checked_add(i64::from(b - b'0'))
        })?;
    Some(if neg { -value } else { value })
}
pub fn date(text: &str) -> Option<String> {
    let t = text.trim();
    if let Ok(d) = time::Date::parse(t, &time::format_description::well_known::Iso8601::DEFAULT) {
        return Some(format!(
            "{:04}-{:02}-{:02}",
            d.year(),
            u8::from(d.month()),
            d.day()
        ));
    }
    if t.len() != 8 || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let y: i32 = t[0..4].parse().ok()?;
    let m: u8 = t[4..6].parse().ok()?;
    let d: u8 = t[6..8].parse().ok()?;
    let month = time::Month::try_from(m).ok()?;
    let date = time::Date::from_calendar_date(y, month, d).ok()?;
    Some(format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    ))
}
