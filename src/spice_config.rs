use anytofire::layout::PLUGIN_DLL;
use roxmltree::{Document, Node};

pub const EMPTY_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<games>\n</games>\n";

pub struct SpiceConfig<'a> {
    xml: &'a str,
    document: Document<'a>,
}

impl<'a> SpiceConfig<'a> {
    pub fn parse(xml: &'a str) -> Result<Self, String> {
        let xml = if xml.trim_start_matches('\u{feff}').trim().is_empty() {
            EMPTY_XML
        } else {
            xml
        };
        let document =
            Document::parse(xml).map_err(|error| format!("Spice XML 无法解析：{error}"))?;
        if !is_tag(document.root_element(), "games") {
            return Err("Spice XML 缺少 games 根节点".into());
        }
        Ok(Self { xml, document })
    }

    fn iidx(&self) -> Option<Node<'_, 'a>> {
        self.document
            .root_element()
            .children()
            .find(|node| is_tag(*node, "game") && node.attribute("name") == Some("Beatmania IIDX"))
    }

    fn options(&self) -> Option<Node<'_, 'a>> {
        self.iidx()?
            .children()
            .find(|node| is_tag(*node, "options"))
    }

    fn hook(&self) -> Option<Node<'_, 'a>> {
        self.options()?
            .children()
            .find(|node| is_tag(*node, "option") && node.attribute("name") == Some("k"))
    }

    pub fn hook_value(&self) -> Option<&str> {
        self.hook()?.attribute("value")
    }

    pub fn plugin_enabled(&self) -> bool {
        self.hook_value()
            .unwrap_or_default()
            .split_ascii_whitespace()
            .any(is_plugin_hook)
    }

    pub fn set_hook(&self, plugin: Option<&str>, remove_legacy: bool) -> String {
        let current = self.hook_value().unwrap_or_default();
        let mut hooks: Vec<_> = current
            .split_ascii_whitespace()
            .filter(|name| {
                !is_plugin_hook(name)
                    && (!remove_legacy || !dll_name(name).eq_ignore_ascii_case("IIDXScore.dll"))
            })
            .collect();
        hooks.extend(plugin);
        let value = hooks.join(" ");
        let mut output = self.xml.to_owned();

        if let Some(option) = self.hook() {
            if let Some(attribute) = option.attribute_node("value") {
                if current != value {
                    output.replace_range(attribute.range_value(), &escape_attribute(&value));
                }
            } else {
                let insertion = option.range().start + "<option".len();
                output.insert_str(
                    insertion,
                    &format!(r#" value="{}""#, escape_attribute(&value)),
                );
            }
            return output;
        }

        let option = format!(r#"<option name="k" value="{}"/>"#, escape_attribute(&value));
        if let Some(options) = self.options() {
            insert_child(&mut output, options, &option);
        } else if let Some(game) = self.iidx() {
            insert_child(&mut output, game, &format!("<options>{option}</options>"));
        } else {
            let root = self.document.root_element();
            insert_child(
                &mut output,
                root,
                &format!(r#"<game name="Beatmania IIDX"><options>{option}</options></game>"#),
            );
        }
        output
    }
}

fn is_tag(node: Node<'_, '_>, name: &str) -> bool {
    node.has_tag_name(name) && node.tag_name().namespace().is_none()
}

fn dll_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn is_plugin_hook(path: &str) -> bool {
    dll_name(path).eq_ignore_ascii_case(PLUGIN_DLL)
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn insert_child(xml: &mut String, parent: Node<'_, '_>, child: &str) {
    let range = parent.range();
    if xml[range.clone()].ends_with("/>") {
        let tag = parent.tag_name().name();
        xml.replace_range(range.end - 2..range.end, &format!(">{child}</{tag}>"));
    } else {
        // A parsed nonempty element ends in its own closing tag.
        let closing = xml[..range.end].rfind("</").unwrap();
        xml.insert_str(closing, child);
    }
}
