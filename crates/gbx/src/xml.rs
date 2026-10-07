//! XML-RPC document encoding and decoding (methodCall / methodResponse).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use base64::Engine as _;
use quick_xml::Reader;
use quick_xml::escape::{escape, unescape};
use quick_xml::events::Event;

use crate::error::{Fault, XmlError};
use crate::value::Value;

/// A decoded `<methodCall>` (sent by the server for callbacks).
#[derive(Debug, Clone, PartialEq)]
pub struct MethodCall {
    pub method: String,
    pub params: Vec<Value>,
}

/// A decoded `<methodResponse>`: either a value or a fault.
pub type MethodResponse = Result<Value, Fault>;

/// Either kind of document the server can send.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Call(MethodCall),
    Response(MethodResponse),
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// Encodes a `<methodCall>` document.
pub fn encode_call(method: &str, params: &[Value]) -> String {
    let mut out = String::with_capacity(128);
    out.push_str(r#"<?xml version="1.0" encoding="UTF-8"?><methodCall><methodName>"#);
    out.push_str(&escape(method));
    out.push_str("</methodName><params>");
    for param in params {
        out.push_str("<param>");
        encode_value(&mut out, param);
        out.push_str("</param>");
    }
    out.push_str("</params></methodCall>");
    out
}

/// Encodes a `<methodResponse>` document (used by tests and the mock server).
pub fn encode_response(response: &MethodResponse) -> String {
    let mut out = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><methodResponse>"#);
    match response {
        Ok(value) => {
            out.push_str("<params><param>");
            encode_value(&mut out, value);
            out.push_str("</param></params>");
        }
        Err(fault) => {
            let mut members = BTreeMap::new();
            members.insert("faultCode".to_owned(), Value::Int(fault.code));
            members.insert(
                "faultString".to_owned(),
                Value::String(fault.message.clone()),
            );
            out.push_str("<fault>");
            encode_value(&mut out, &Value::Struct(members));
            out.push_str("</fault>");
        }
    }
    out.push_str("</methodResponse>");
    out
}

fn encode_value(out: &mut String, value: &Value) {
    out.push_str("<value>");
    match value {
        Value::Int(i) => {
            let _ = write!(out, "<int>{i}</int>");
        }
        Value::Bool(b) => {
            let _ = write!(out, "<boolean>{}</boolean>", u8::from(*b));
        }
        Value::String(s) => {
            out.push_str("<string>");
            out.push_str(&escape(s.as_str()));
            out.push_str("</string>");
        }
        Value::Double(d) => {
            let _ = write!(out, "<double>{d}</double>");
        }
        Value::DateTime(s) => {
            out.push_str("<dateTime.iso8601>");
            out.push_str(&escape(s.as_str()));
            out.push_str("</dateTime.iso8601>");
        }
        Value::Base64(bytes) => {
            out.push_str("<base64>");
            out.push_str(&base64::engine::general_purpose::STANDARD.encode(bytes));
            out.push_str("</base64>");
        }
        Value::Array(items) => {
            out.push_str("<array><data>");
            for item in items {
                encode_value(out, item);
            }
            out.push_str("</data></array>");
        }
        Value::Struct(members) => {
            out.push_str("<struct>");
            for (name, member) in members {
                out.push_str("<member><name>");
                out.push_str(&escape(name.as_str()));
                out.push_str("</name>");
                encode_value(out, member);
                out.push_str("</member>");
            }
            out.push_str("</struct>");
        }
        Value::Nil => out.push_str("<nil/>"),
    }
    out.push_str("</value>");
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Minimal element tree: XML-RPC documents are small, a tree keeps the decoder simple.
#[derive(Debug, Default)]
struct Element {
    name: String,
    text: String,
    children: Vec<Element>,
}

impl Element {
    fn child(&self, name: &str) -> Result<&Element, XmlError> {
        self.children
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| XmlError::Invalid(format!("<{}> has no <{name}> child", self.name)))
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

fn parse_tree(xml: &str) -> Result<Element, XmlError> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;

    let close = |stack: &mut Vec<Element>, root: &mut Option<Element>, element: Element| match stack
        .last_mut()
    {
        Some(parent) => parent.children.push(element),
        None => *root = Some(element),
    };

    loop {
        match reader.read_event()? {
            Event::Start(start) => stack.push(Element {
                name: start.local_name().as_ref().to_owned(),
                ..Element::default()
            }),
            Event::Empty(start) => {
                let element = Element {
                    name: start.local_name().as_ref().to_owned(),
                    ..Element::default()
                };
                close(&mut stack, &mut root, element);
            }
            Event::End(_) => {
                let element = stack
                    .pop()
                    .ok_or_else(|| XmlError::Invalid("unbalanced end tag".to_owned()))?;
                close(&mut stack, &mut root, element);
            }
            Event::Text(text) => {
                if let Some(current) = stack.last_mut() {
                    current.text.push_str(&text.xml10_content());
                }
            }
            Event::CData(data) => {
                if let Some(current) = stack.last_mut() {
                    current.text.push_str(&data.into_inner());
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(current) = stack.last_mut() {
                    match reference.resolve_char_ref().map_err(XmlError::Syntax)? {
                        Some(ch) => current.text.push(ch),
                        None => {
                            let entity = format!("&{};", reference.into_inner());
                            let resolved = unescape(&entity).map_err(|_| XmlError::BadLiteral {
                                kind: "entity",
                                literal: entity.clone(),
                            })?;
                            current.text.push_str(&resolved);
                        }
                    }
                }
            }
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }

    if !stack.is_empty() {
        return Err(XmlError::Invalid("unexpected end of document".to_owned()));
    }
    root.ok_or_else(|| XmlError::Invalid("empty document".to_owned()))
}

/// Decodes any document sent by the server.
pub fn decode(xml: &str) -> Result<Message, XmlError> {
    let root = parse_tree(xml)?;
    match root.name.as_str() {
        "methodCall" => decode_call_tree(&root).map(Message::Call),
        "methodResponse" => decode_response_tree(&root).map(Message::Response),
        other => Err(XmlError::Invalid(format!(
            "unexpected root element <{other}>"
        ))),
    }
}

/// Decodes a `<methodCall>` document.
pub fn decode_call(xml: &str) -> Result<MethodCall, XmlError> {
    match decode(xml)? {
        Message::Call(call) => Ok(call),
        Message::Response(_) => Err(XmlError::Invalid("expected <methodCall>".to_owned())),
    }
}

/// Decodes a `<methodResponse>` document.
pub fn decode_response(xml: &str) -> Result<MethodResponse, XmlError> {
    match decode(xml)? {
        Message::Response(response) => Ok(response),
        Message::Call(_) => Err(XmlError::Invalid("expected <methodResponse>".to_owned())),
    }
}

fn decode_params(params: Option<&Element>) -> Result<Vec<Value>, XmlError> {
    let Some(params) = params else {
        return Ok(Vec::new());
    };
    params
        .children_named("param")
        .map(|param| decode_value(param.child("value")?))
        .collect()
}

fn decode_call_tree(root: &Element) -> Result<MethodCall, XmlError> {
    let method = root.child("methodName")?.text.trim().to_owned();
    let params = decode_params(root.children.iter().find(|c| c.name == "params"))?;
    Ok(MethodCall { method, params })
}

fn decode_response_tree(root: &Element) -> Result<MethodResponse, XmlError> {
    if let Some(fault) = root.children.iter().find(|c| c.name == "fault") {
        let value = decode_value(fault.child("value")?)?;
        let members = value
            .as_struct()
            .ok_or_else(|| XmlError::Invalid("fault is not a struct".to_owned()))?;
        let code = members
            .get("faultCode")
            .and_then(Value::as_i32)
            .unwrap_or(0);
        let message = members
            .get("faultString")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        return Ok(Err(Fault { code, message }));
    }
    let mut params = decode_params(root.children.iter().find(|c| c.name == "params"))?;
    // A response always carries exactly one value; tolerate an empty <params/>.
    Ok(Ok(if params.is_empty() {
        Value::Nil
    } else {
        params.swap_remove(0)
    }))
}

fn decode_value(value: &Element) -> Result<Value, XmlError> {
    let Some(typed) = value.children.first() else {
        // No type element: the content is a string.
        return Ok(Value::String(value.text.clone()));
    };
    let text = typed.text.as_str();
    let bad = |kind: &'static str| XmlError::BadLiteral {
        kind,
        literal: text.to_owned(),
    };
    Ok(match typed.name.as_str() {
        "i4" | "int" => Value::Int(text.trim().parse().map_err(|_| bad("int"))?),
        "boolean" => Value::Bool(match text.trim() {
            "1" | "true" => true,
            "0" | "false" => false,
            _ => return Err(bad("boolean")),
        }),
        "string" => Value::String(text.to_owned()),
        "double" => Value::Double(text.trim().parse().map_err(|_| bad("double"))?),
        "dateTime.iso8601" => Value::DateTime(text.trim().to_owned()),
        "base64" => {
            let compact: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            Value::Base64(
                base64::engine::general_purpose::STANDARD
                    .decode(compact)
                    .map_err(|_| bad("base64"))?,
            )
        }
        "array" => Value::Array(
            typed
                .child("data")?
                .children_named("value")
                .map(decode_value)
                .collect::<Result<_, _>>()?,
        ),
        "struct" => Value::Struct(
            typed
                .children_named("member")
                .map(|member| {
                    let name = member.child("name")?.text.clone();
                    Ok((name, decode_value(member.child("value")?)?))
                })
                .collect::<Result<_, XmlError>>()?,
        ),
        "nil" => Value::Nil,
        other => return Err(XmlError::Invalid(format!("unknown value type <{other}>"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_simple_boolean_response() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<methodResponse>
<params>
<param><value><boolean>1</boolean></value></param>
</params>
</methodResponse>"#;
        assert_eq!(decode_response(xml).unwrap(), Ok(Value::Bool(true)));
    }

    #[test]
    fn decodes_fault() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<methodResponse>
<fault>
<value><struct>
<member><name>faultCode</name><value><int>-1000</int></value></member>
<member><name>faultString</name><value><string>Permission denied.</string></value></member>
</struct></value>
</fault>
</methodResponse>"#;
        assert_eq!(
            decode_response(xml).unwrap(),
            Err(Fault {
                code: -1000,
                message: "Permission denied.".to_owned()
            })
        );
    }

    /// Shape of `GetVersion` as documented by Nadeo (values are illustrative).
    #[test]
    fn decodes_struct_response() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<methodResponse><params><param><value><struct>
<member><name>Name</name><value><string>Trackmania</string></value></member>
<member><name>TitleId</name><value><string>Trackmania</string></value></member>
<member><name>Version</name><value><string>3.3.0</string></value></member>
<member><name>Build</name><value><string>2026-09-01_12_00</string></value></member>
<member><name>ApiVersion</name><value><string>2023-04-24</string></value></member>
</struct></value></param></params></methodResponse>"#;
        let value = decode_response(xml).unwrap().unwrap();
        let s = value.as_struct().unwrap();
        assert_eq!(s["Name"], Value::from("Trackmania"));
        assert_eq!(s["ApiVersion"], Value::from("2023-04-24"));
    }

    #[test]
    fn decodes_player_connect_callback() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<methodCall>
<methodName>ManiaPlanet.PlayerConnect</methodName>
<params>
<param><value><string>xX_player_Xx</string></value></param>
<param><value><boolean>0</boolean></value></param>
</params>
</methodCall>"#;
        let call = decode_call(xml).unwrap();
        assert_eq!(call.method, "ManiaPlanet.PlayerConnect");
        assert_eq!(
            call.params,
            vec![Value::from("xX_player_Xx"), Value::Bool(false)]
        );
    }

    #[test]
    fn decodes_mode_script_callback_with_escaped_json() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<methodCall><methodName>ManiaPlanet.ModeScriptCallbackArray</methodName><params>
<param><value><string>Trackmania.Event.WayPoint</string></value></param>
<param><value><array><data><value><string>{&quot;login&quot;: &quot;abc&quot;, &quot;racetime&quot;: 12345, &quot;name&quot;: &quot;&lt;b&gt; &amp; &#233;&quot;}</string></value></data></array></value></param>
</params></methodCall>"#;
        let call = decode_call(xml).unwrap();
        assert_eq!(call.method, "ManiaPlanet.ModeScriptCallbackArray");
        let payload = call.params[1].as_array().unwrap()[0].as_str().unwrap();
        assert_eq!(
            payload,
            r#"{"login": "abc", "racetime": 12345, "name": "<b> & é"}"#
        );
    }

    #[test]
    fn untyped_value_is_a_string_and_whitespace_is_kept() {
        let xml = "<methodResponse><params><param><value>  hello  </value></param></params></methodResponse>";
        assert_eq!(decode_response(xml).unwrap(), Ok(Value::from("  hello  ")));
    }

    #[test]
    fn empty_string_and_empty_array() {
        let xml = "<methodResponse><params><param><value><array><data><value><string/></value></data></array></value></param></params></methodResponse>";
        assert_eq!(
            decode_response(xml).unwrap(),
            Ok(Value::Array(vec![Value::from("")]))
        );
        let xml = "<methodResponse><params><param><value><array><data/></array></value></param></params></methodResponse>";
        assert_eq!(decode_response(xml).unwrap(), Ok(Value::Array(vec![])));
    }

    #[test]
    fn rejects_bad_literals() {
        let xml = "<methodResponse><params><param><value><int>abc</int></value></param></params></methodResponse>";
        assert!(matches!(
            decode_response(xml),
            Err(XmlError::BadLiteral { kind: "int", .. })
        ));
    }

    #[test]
    fn rejects_truncated_document() {
        let xml = "<methodResponse><params><param><value><int>1</int>";
        assert!(decode_response(xml).is_err());
    }

    #[test]
    fn encode_then_decode_round_trip() {
        let mut members = BTreeMap::new();
        members.insert("S_TimeLimit".to_owned(), Value::Int(300));
        members.insert("S_UseTieBreak".to_owned(), Value::Bool(false));
        let params = vec![
            Value::from("a <tricky> & \"string\""),
            Value::Double(0.25),
            Value::Base64(vec![0, 1, 2, 255]),
            Value::Array(vec![Value::Int(-1), Value::from("x")]),
            Value::Struct(members),
        ];
        let xml = encode_call("SetModeScriptSettings", &params);
        let call = decode_call(&xml).unwrap();
        assert_eq!(call.method, "SetModeScriptSettings");
        assert_eq!(call.params, params);
    }

    #[test]
    fn encodes_authenticate_exactly() {
        let xml = encode_call(
            "Authenticate",
            &[Value::from("SuperAdmin"), Value::from("p&ss")],
        );
        assert_eq!(
            xml,
            r#"<?xml version="1.0" encoding="UTF-8"?><methodCall><methodName>Authenticate</methodName><params><param><value><string>SuperAdmin</string></value></param><param><value><string>p&amp;ss</string></value></param></params></methodCall>"#
        );
    }

    #[test]
    fn response_round_trip_including_fault() {
        let ok: MethodResponse = Ok(Value::Array(vec![Value::Int(1)]));
        assert_eq!(decode_response(&encode_response(&ok)).unwrap(), ok);
        let fault: MethodResponse = Err(Fault {
            code: -1000,
            message: "Login unknown.".to_owned(),
        });
        assert_eq!(decode_response(&encode_response(&fault)).unwrap(), fault);
    }
}
