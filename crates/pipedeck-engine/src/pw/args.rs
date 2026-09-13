//! Writing the SPA JSON that PipeWire modules take as arguments.
//!
//! It is JSON's shape with looser rules: keys are bare words, strings are
//! quoted only when they need to be. Only what the modules we load actually
//! need is here, which is objects, arrays, strings and raw literals.

/// A value in a module's arguments.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    /// Quoted and escaped.
    Str(String),
    /// Written as given: numbers, booleans, bare words.
    Raw(String),
    /// `{ key = value ... }`, in the order given.
    Dict(Vec<(String, Val)>),
    /// `[ value ... ]`.
    Array(Vec<Val>),
}

impl From<&str> for Val {
    fn from(value: &str) -> Self {
        Val::Str(value.to_owned())
    }
}

impl From<String> for Val {
    fn from(value: String) -> Self {
        Val::Str(value)
    }
}

impl From<bool> for Val {
    fn from(value: bool) -> Self {
        Val::Raw(value.to_string())
    }
}

impl From<f32> for Val {
    fn from(value: f32) -> Self {
        Val::Raw(format!("{value}"))
    }
}

impl Val {
    /// A dictionary from anything that names its entries.
    pub fn dict<K: Into<String>, I: IntoIterator<Item = (K, Val)>>(entries: I) -> Self {
        Val::Dict(
            entries
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    pub fn array<I: IntoIterator<Item = Val>>(values: I) -> Self {
        Val::Array(values.into_iter().collect())
    }

    fn write(&self, out: &mut String) {
        match self {
            Val::Str(text) => {
                out.push('"');
                for character in text.chars() {
                    match character {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        other => out.push(other),
                    }
                }
                out.push('"');
            }
            Val::Raw(text) => out.push_str(text),
            Val::Dict(entries) => {
                out.push_str("{ ");
                for (key, value) in entries {
                    out.push_str(key);
                    out.push_str(" = ");
                    value.write(out);
                    out.push(' ');
                }
                out.push('}');
            }
            Val::Array(values) => {
                out.push_str("[ ");
                for value in values {
                    value.write(out);
                    out.push(' ');
                }
                out.push(']');
            }
        }
    }
}

/// Render the top-level arguments of a module.
pub fn render(entries: Vec<(&str, Val)>) -> String {
    let mut out = String::new();
    Val::dict(entries).write(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dictionary_reads_back_as_written() {
        let args = render(vec![
            ("node.name", Val::from("pipedeck.1")),
            ("passive", Val::from(true)),
            (
                "audio.position",
                Val::array([Val::Raw("FL".into()), Val::Raw("FR".into())]),
            ),
            (
                "capture.props",
                Val::dict([("target.object", Val::from("a \"sink\""))]),
            ),
        ]);
        assert_eq!(
            args,
            "{ node.name = \"pipedeck.1\" passive = true \
             audio.position = [ FL FR ] \
             capture.props = { target.object = \"a \\\"sink\\\"\" } }"
        );
    }

    #[test]
    fn an_empty_collection_still_closes() {
        assert_eq!(render(vec![("nodes", Val::array([]))]), "{ nodes = [ ] }");
    }
}
