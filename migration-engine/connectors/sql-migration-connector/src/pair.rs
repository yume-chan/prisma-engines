use crate::SqlDatabaseSchema;
use sql_schema_describer::{
    walkers::{ColumnWalker, EnumWalker, ForeignKeyWalker, IndexWalker, SqlSchemaExt, TableWalker},
    ColumnId, EnumId, ForeignKeyId, IndexId, SqlSchema, TableId,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Pair<T> {
    pub previous: T,
    pub next: T,
}

impl<T> Pair<T> {
    pub(crate) fn new(previous: T, next: T) -> Self {
        Pair { previous, next }
    }

    pub(crate) fn as_ref(&self) -> Pair<&T> {
        Pair {
            previous: &self.previous,
            next: &self.next,
        }
    }

    /// Map each element to an iterator, and zip the two iterators into an iterator over pairs.
    pub(crate) fn interleave<F, I, O>(&self, f: F) -> impl Iterator<Item = Pair<O>>
    where
        I: IntoIterator<Item = O>,
        F: Fn(&T) -> I,
    {
        f(&self.previous)
            .into_iter()
            .zip(f(&self.next).into_iter())
            .map(Pair::from)
    }

    pub(crate) fn into_tuple(self) -> (T, T) {
        (self.previous, self.next)
    }

    pub(crate) fn map<U>(self, f: impl Fn(T) -> U) -> Pair<U> {
        Pair {
            previous: f(self.previous),
            next: f(self.next),
        }
    }

    pub(crate) fn zip<U>(self, other: Pair<U>) -> Pair<(T, U)> {
        Pair::new((self.previous, other.previous), (self.next, other.next))
    }

    pub(crate) fn combine<U>(self, other: Pair<U>) -> Pair<(T, U)> {
        Pair {
            previous: (self.previous, other.previous),
            next: (self.next, other.next),
        }
    }
}

impl<T> Pair<Option<T>> {
    pub(crate) fn transpose(self) -> Option<Pair<T>> {
        match (self.previous, self.next) {
            (Some(previous), Some(next)) => Some(Pair { previous, next }),
            _ => None,
        }
    }
}

impl<'a> Pair<&'a SqlDatabaseSchema> {
    pub(crate) fn enums(&self, ids: Pair<EnumId>) -> Pair<EnumWalker<'a>> {
        Pair::new(self.previous.walk_enum(ids.previous), self.next.walk_enum(ids.next))
    }

    pub(crate) fn tables(&self, table_ids: &Pair<TableId>) -> Pair<TableWalker<'a>> {
        Pair::new(
            self.previous.table_walker_at(table_ids.previous),
            self.next.table_walker_at(table_ids.next),
        )
    }
}

impl<'a> Pair<&'a SqlDatabaseSchema> {
    pub(crate) fn columns(&self, column_ids: Pair<ColumnId>) -> Pair<ColumnWalker<'a>> {
        self.zip(column_ids).map(|(s, c)| s.describer_schema.walk_column(c))
    }
}

impl<'a> Pair<&'a SqlSchema> {
    pub(crate) fn enums(self, ids: Pair<EnumId>) -> Pair<EnumWalker<'a>> {
        Pair::new(self.previous.walk_enum(ids.previous), self.next.walk_enum(ids.next))
    }

    pub(crate) fn tables(self, table_ids: Pair<TableId>) -> Pair<TableWalker<'a>> {
        self.zip(table_ids).map(|(s, t)| s.table_walker_at(t))
    }

    pub(crate) fn columns(self, column_ids: Pair<ColumnId>) -> Pair<ColumnWalker<'a>> {
        self.zip(column_ids).map(|(s, c)| s.walk_column(c))
    }

    pub(crate) fn foreign_keys(self, fk_ids: Pair<ForeignKeyId>) -> Pair<ForeignKeyWalker<'a>> {
        self.zip(fk_ids).map(|(s, id)| s.walk_foreign_key(id))
    }
}

impl<'a> Pair<TableWalker<'a>> {
    pub(crate) fn indexes(&self, index_indexes: &Pair<IndexId>) -> Pair<IndexWalker<'a>> {
        self.as_ref().zip(index_indexes.as_ref()).map(|(t, i)| t.index_at(*i))
    }
}

impl<T> From<(T, T)> for Pair<T> {
    fn from((previous, next): (T, T)) -> Self {
        Pair { previous, next }
    }
}
