use std::borrow::Cow;

enum IndexType {
    Normal,
    Unique,
    Fulltext,
}

pub struct Index<'a> {
    r#type: IndexType,
    name: Option<Cow<'a, str>>,
    map: Option<Cow<'a, str>>,
}

impl<'a> Index<'a> {
    pub fn new() -> Self {
        Self {
            r#type: IndexType::Normal,
            name: None,
            map: None,
        }
    }

    pub fn unique() -> Self {
        Self {
            r#type: IndexType::Unique,
            name: None,
            map: None,
        }
    }

    pub fn fulltext() -> Self {
        Self {
            r#type: IndexType::Fulltext,
            name: None,
            map: None,
        }
    }

    pub fn set_name(&mut self, name: impl Into<Cow<'a, str>>) {
        self.name = Some(name.into());
    }

    pub fn set_map(&mut self, name: impl Into<Cow<'a, str>>) {
        self.map = Some(name.into());
    }
}

pub enum IndexFieldSort {
    Ascending,
    Descending,
}

impl Default for IndexFieldSort {
    fn default() -> Self {
        Self::Ascending
    }
}

pub struct IndexField<'a> {
    path: Cow<'a, str>,
    sort: IndexFieldSort,
}

impl<'a> IndexField<'a> {
    pub fn new(path: impl Into<Cow<'a, str>>) -> Self {
        Self {
            path: path.into(),
            sort: IndexFieldSort::Ascending,
        }
    }

    pub fn sort(&mut self, sort: IndexFieldSort) {
        self.sort = sort;
    }
}
