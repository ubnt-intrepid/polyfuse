pub(crate) trait IteratorJoinExt: Iterator
where
    Self::Item: AsRef<str>,
{
    fn join(self, sep: &str) -> String
    where
        Self: Sized,
    {
        self.enumerate().fold(String::new(), |mut acc, (i, elem)| {
            if i > 0 {
                acc.push_str(sep);
            }
            acc.push_str(elem.as_ref().trim());
            acc
        })
    }
}

impl<I> IteratorJoinExt for I
where
    I: Iterator,
    I::Item: AsRef<str>,
{
}
