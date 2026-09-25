use std::marker::PhantomData;

use crate::crdt::state::{Tracked, markers, value};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Counter<T> {
    count: u32,
    operation: PhantomData<T>,
}

impl<T> Counter<T> {
    pub(crate) fn increment(&mut self) {
        self.count += 1;
    }

    pub(crate) const fn count(&self) -> u32 {
        self.count
    }
}

impl<T> Default for Counter<T> {
    fn default() -> Self {
        Self {
            count: 0,
            operation: PhantomData,
        }
    }
}

impl<T> From<u32> for Counter<T> {
    fn from(count: u32) -> Self {
        Self {
            count,
            operation: PhantomData,
        }
    }
}

pub trait Conflict {
    fn conflicts_with(&self, other: &Tracked<value::Value<'_>>) -> bool;
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Read;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExistenceCheck;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Write;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Delta;

pub enum OperationCounter {
    Read(Counter<Read>),
    ExistenceCheck(Counter<ExistenceCheck>),
    Write(Counter<Write>),
    Delta(Counter<Delta>),
}
