use super::value::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Marker {
    Missing,
    Deleted,
    Default,
    Stripped,
}

pub fn from_value(value: &Value<'_>) -> Value<'static> {
    match value {
        Value::Numeric(_) => value.clone().into_owned(),
        Value::Marker(Marker::Missing) => Value::Marker(Marker::Missing),
        Value::Marker(Marker::Deleted) => Value::Marker(Marker::Deleted),
        Value::Marker(Marker::Default) => Value::Marker(Marker::Default),
        _ => Value::Marker(Marker::Stripped),
    }
}

pub fn is_deleted(value: &Value<'_>) -> bool {
    matches!(value, Value::Marker(Marker::Deleted))
}

pub fn is_default(value: &Value<'_>) -> bool {
    matches!(value, Value::Marker(Marker::Default))
}

pub fn is_stripped(value: &Value<'_>) -> bool {
    matches!(value, Value::Marker(Marker::Stripped))
}
