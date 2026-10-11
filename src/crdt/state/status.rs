use super::value::{CrdtValue, ValueSnapshots};

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
    Value(CrdtValue<'static>),
}

/// Strips the given `ValueSnapshots` by converting them into `Status` representations.
/// This is mainly for reducing the memory footprint when checking conflicts.
pub fn reduce_to_statuses(
    value: &ValueSnapshots<CrdtValue<'_>, CrdtValue<'_>>,
) -> ValueSnapshots<Status, Status> {
    let original = match &value.original {
        CrdtValue::None => Status::Tag(Tag::Missing),
        _ => Status::Tag(Tag::Stripped),
    };

    let current = match &value.current {
        CrdtValue::Numeric(value) => Status::Value(CrdtValue::Numeric(value.clone().into_owned())),
        CrdtValue::Bytes(value) => {
            if value.delta.is_none() {
                Status::Tag(Tag::Default)
            } else {
                Status::Tag(Tag::Stripped)
            }
        }
        CrdtValue::U64Set(value) => {
            if value.delta.is_none() {
                Status::Tag(Tag::Default) // no pending changes
            } else {
                Status::Tag(Tag::Stripped) // has pending changes, strip the details.
            }
        }
        CrdtValue::None => {
            if original == Status::Tag(Tag::Missing) {
                Status::Tag(Tag::Missing) // Original value was missing too.
            } else {
                Status::Tag(Tag::Deleted) // Original was present, current missing. A deletion.
            }
        }
    };
    ValueSnapshots { original, current }
}
