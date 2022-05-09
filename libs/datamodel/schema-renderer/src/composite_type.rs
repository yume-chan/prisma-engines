use std::borrow::Cow;

pub struct CompositeType<'a> {
    name: Cow<'a, str>,
}

impl<'a> CompositeType<'a> {
    pub fn new(name: impl Into<Cow<'a, str>>) -> Self {
        Self { name: name.into() }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}
