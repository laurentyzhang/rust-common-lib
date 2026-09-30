use super::value::{Value, Values};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tag {
    Missing,
    Deleted,
    Default,
    Stripped,
}

#[derive(Clone, PartialEq)]
pub enum Status {
    Tag(Tag),
    Value(Value<'static>),
}

/// Strips the given `Values` by converting them into `Status` representations.
/// This is mainly for reducing the memory footprint when checking conflicts.
pub fn strip(value: &Values<Value<'_>, Value<'_>>) -> Values<Status, Status> {
    let original = match &value.original {
        Value::None => Status::Tag(Tag::Missing),
        _ => Status::Tag(Tag::Stripped),
    };

    let current = match &value.current {
        Value::Numeric(value) => Status::Value(Value::Numeric(value.clone().into_owned())),
        Value::Bytes(value) => {
            if value.delta.is_none() {
                Status::Tag(Tag::Default)
            } else {
                Status::Tag(Tag::Stripped)
            }
        }
        Value::U64Set(value) => {
            if value.delta.is_none() {
                Status::Tag(Tag::Default) // no pending changes
            } else {
                Status::Tag(Tag::Stripped) // has pending changes, strip the details.
            }
        }
        Value::None => {
            if original == Status::Tag(Tag::Missing) {
                Status::Tag(Tag::Missing) // Original value was missing too.
            } else {
                Status::Tag(Tag::Deleted) // Original was present, current missing. A deletion.
            }
        }
    };
    Values { original, current }
}
